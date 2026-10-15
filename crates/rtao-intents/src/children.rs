// Derived from RaoFoundation/subtensor sdk/python/bittensor/intents/children.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Child hotkey shares and delegate takes with Python proportion conversion.

use rtao_client::{View, ViewExt};
use rtao_primitives::{AccountId, PerU16, Ss58Address, hyperparams, pyfmt};
use rtao_runtime::{calls::subtensor_module, storage::subtensor_module as st};
use rtao_tx::{BuildCx, Built, Intent, IntentError, PreflightCx};
use serde_json::{Value, json};

pub use crate::{DecreaseTake, IncreaseTake, SetChildkeyTake, SetChildren, SetTake};

fn proportion(value: &Value, denominator: u64, label: &str) -> Result<u128, IntentError> {
    let converted = match value {
        Value::Bool(value) => hyperparams::proportion_to_raw_with_label(*value, denominator, label),
        Value::String(value) => {
            hyperparams::proportion_to_raw_with_label(value.as_str(), denominator, label)
        }
        Value::Number(value) if value.as_str().contains(['.', 'e', 'E']) => {
            let number = pyfmt::parse_float(value.as_str())
                .ok_or_else(|| IntentError::validation(format!("{label} value is out of range")))?;
            hyperparams::proportion_to_raw_with_label(number, denominator, label)
        }
        Value::Number(value) => {
            let number = pyfmt::parse_integer(value.as_str())
                .ok_or_else(|| IntentError::validation(format!("{label} value is out of range")))?;
            hyperparams::proportion_to_raw_with_label(number, denominator, label)
        }
        _ => {
            return Err(IntentError::validation(format!(
                "{label} must be a number or string"
            )));
        }
    };
    converted.map_err(|error| IntentError::validation(error.to_string()))
}

/// Convert a human fraction or raw integer to a checked native delegate take.
#[doc(alias = "bittensor.intents.children.take_to_u16")]
pub fn take_to_u16(value: &Value) -> Result<u16, IntentError> {
    u16::try_from(proportion(value, u64::from(u16::MAX), "take")?)
        .map_err(|_| IntentError::validation("take value is out of range"))
}

/// Typed conversion used by the generated runtime-call bindings.
pub(crate) fn take(value: &Value) -> Result<PerU16, IntentError> {
    take_to_u16(value).map(PerU16)
}

/// Store Python's constructor-normalized raw take in the generated carrier.
pub(crate) fn canonical_take(value: &mut Value) -> Result<(), IntentError> {
    *value = json!(take_to_u16(value)?);
    Ok(())
}

fn take_summary(value: &Value) -> String {
    take_to_u16(value).map_or_else(|_| value.to_string(), |raw| raw.to_string())
}

/// Normalize typed callers' JSON as well as tool-parser inputs.
pub(crate) fn take_json(op: &str, intent: &impl serde::Serialize, take: &Value) -> Value {
    let mut value = crate::args_with_op(op, intent);
    if let Ok(raw) = take_to_u16(take) {
        value["take"] = json!(raw);
    }
    value
}

// Refusal diagnostics have bounded recursion and work even for programmatic
// invalid JSON. Ordinary short entries retain Python's repr punctuation.
fn entry_repr(value: &Value, budget: &mut usize, depth: usize) -> String {
    if *budget == 0 || depth == 8 {
        return "...".into();
    }
    *budget -= 1;
    match value {
        Value::Null => "None".into(),
        Value::Bool(value) => if *value { "True" } else { "False" }.into(),
        Value::String(value) if value.len() <= 2048 => pyfmt::string_repr(value),
        Value::String(_) => "'...'".into(),
        Value::Number(value) => value.to_string(),
        Value::Array(values) => {
            let mut rendered = Vec::new();
            for value in values {
                rendered.push(entry_repr(value, budget, depth + 1));
                if *budget == 0 {
                    break;
                }
            }
            format!("[{}]", rendered.join(", "))
        }
        Value::Object(values) => {
            let mut rendered = Vec::new();
            for (key, value) in values {
                let key = if key.len() <= 2048 {
                    pyfmt::string_repr(key)
                } else {
                    "'...'".into()
                };
                rendered.push(format!("{key}: {}", entry_repr(value, budget, depth + 1)));
                if *budget == 0 {
                    break;
                }
            }
            format!("{{{}}}", rendered.join(", "))
        }
    }
}

#[derive(Clone, Copy)]
enum ChildPart<'a> {
    Json(&'a Value),
    Text(&'a str),
}

impl ChildPart<'_> {
    fn proportion(self) -> Result<u128, IntentError> {
        match self {
            Self::Json(value) => proportion(value, u64::MAX, "child proportion"),
            Self::Text(value) => {
                hyperparams::proportion_to_raw_with_label(value, u64::MAX, "child proportion")
                    .map_err(|error| IntentError::validation(error.to_string()))
            }
        }
    }

    fn account(self) -> Result<AccountId, IntentError> {
        let text = match self {
            Self::Json(Value::String(value)) => value.as_str(),
            Self::Text(value) => value,
            Self::Json(value) => {
                return Err(rtao_core::CoreError::Codec(format!(
                    "cannot interpret {value} as an account id"
                ))
                .into());
            }
        };
        // The pinned native AccountId32 encoder accepts SS58 and 0x-hex
        // strings. Its raw Bytes alternative has no serde_json carrier;
        // Python lists remain native Lists and are not accepted as accounts.
        if text.starts_with("0x") {
            let raw = hex::decode(text.trim_start_matches("0x")).map_err(|error| {
                rtao_core::CoreError::Codec(format!("bad hex account id: {error}"))
            })?;
            let bytes = raw
                .as_slice()
                .try_into()
                .map_err(|_| rtao_core::CoreError::Codec("account id must be 32 bytes".into()))?;
            Ok(AccountId(bytes))
        } else {
            Ok(text.parse::<Ss58Address>()?.account())
        }
    }
}

fn child_pair(entry: &Value) -> Result<[ChildPart<'_>; 2], IntentError> {
    // Python unpacks each iterable rather than requiring a list or tuple.
    // serde_json maps expose their retained key order; any insertion order
    // discarded before this function cannot be reconstructed here.
    let pair = match entry {
        Value::Array(pair) if pair.len() == 2 => {
            Some([ChildPart::Json(&pair[0]), ChildPart::Json(&pair[1])])
        }
        Value::Object(map) if map.len() == 2 => {
            let mut keys = map.keys();
            match (keys.next(), keys.next()) {
                (Some(first), Some(second)) => {
                    Some([ChildPart::Text(first), ChildPart::Text(second)])
                }
                _ => None,
            }
        }
        Value::String(text) => {
            let mut characters = text.char_indices();
            match (characters.next(), characters.next(), characters.next()) {
                (Some(_), Some((boundary, _)), None) => Some([
                    ChildPart::Text(&text[..boundary]),
                    ChildPart::Text(&text[boundary..]),
                ]),
                _ => None,
            }
        }
        _ => None,
    };
    pair.ok_or_else(|| {
        IntentError::validation(format!(
            "each child entry must be a [proportion, hotkey_ss58] pair; got {}",
            entry_repr(entry, &mut 32, 0)
        ))
    })
}

fn normalized_proportions(values: &[Value]) -> Result<Vec<u64>, IntentError> {
    let mut normalized = Vec::with_capacity(values.len());
    let mut total = 0u128;
    for entry in values {
        let raw = child_pair(entry)?[0].proportion()?;
        total = total
            .checked_add(raw)
            .ok_or_else(|| IntentError::validation("child proportion total is out of range"))?;
        normalized.push(raw);
    }
    if total > u128::from(u64::MAX) {
        let numerator = pyfmt::parse_integer(&total.to_string())
            .ok_or_else(|| IntentError::validation("child proportion total is out of range"))?;
        let ratio = rtao_primitives::ratio::integer_division_big(&numerator, u128::from(u64::MAX))
            .ok_or_else(|| IntentError::validation("child proportion total is out of range"))?;
        return Err(IntentError::validation(format!(
            "child proportions sum to {} of the parent's stake weight; together they must not exceed 1.0",
            pyfmt::fmt_g4(ratio)
        )));
    }
    normalized
        .into_iter()
        .map(|raw| {
            u64::try_from(raw)
                .map_err(|_| IntentError::validation("child proportion is out of range"))
        })
        .collect()
}

/// Runtime conversion, retaining input ordering and duplicate child accounts.
pub(crate) fn children(values: &[Value]) -> Result<Vec<(u64, AccountId)>, IntentError> {
    normalized_proportions(values)?
        .into_iter()
        .zip(values)
        .map(|(raw, entry)| Ok((raw, child_pair(entry)?[1].account()?)))
        .collect()
}

/// Normalize Python's iterable entries without changing child account values.
pub(crate) fn canonical_children(values: &mut [Value]) -> Result<(), IntentError> {
    let normalized = normalized_proportions(values)?;
    for (entry, raw) in values.iter_mut().zip(normalized) {
        // All entries have been checked, so an invalid later share cannot
        // leave earlier entries normalized. Move the child instead of cloning
        // arbitrary JSON that Python defers to runtime account validation.
        let child = match std::mem::take(entry) {
            Value::Array(mut pair) => pair.pop(),
            Value::Object(map) => map.into_iter().nth(1).map(|(key, _)| Value::String(key)),
            Value::String(text) => text
                .chars()
                .nth(1)
                .map(|character| Value::String(character.to_string())),
            _ => None,
        }
        .ok_or_else(|| IntentError::validation("child entry lost its second item"))?;
        *entry = Value::Array(vec![json!(raw), child]);
    }
    Ok(())
}

/// Normalize typed callers' share presentation using the same constructor path.
pub(crate) fn children_json(intent: &SetChildren) -> Value {
    let mut value = crate::args_with_op("set_children", intent);
    if let Some(children) = value.get_mut("children").and_then(Value::as_array_mut) {
        let _ = canonical_children(children);
    }
    value
}

impl Intent for SetChildren {
    fn summary(&self) -> String {
        format!(
            "set {} child hotkeys on netuid {}",
            self.children.len(),
            self.netuid
        )
    }
    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        crate::generated::passthrough::set_children(self, cx).await
    }
    fn validate(&self) -> Result<(), IntentError> {
        // Python normalizes shares at construction and resolves child
        // addresses only when the runtime call is composed.
        normalized_proportions(&self.children)?;
        Ok(())
    }
    async fn warnings<V: View + ?Sized>(
        &self,
        _cx: &PreflightCx<'_, V>,
    ) -> Result<Vec<String>, IntentError> {
        Ok(vec![
            "delegates a share of this hotkey's stake weight to the children".into(),
        ])
    }
}

impl Intent for SetChildkeyTake {
    fn summary(&self) -> String {
        format!(
            "set childkey take to {}/65535 on netuid {}",
            take_summary(&self.take),
            self.netuid
        )
    }
    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        crate::generated::passthrough::set_childkey_take(self, cx).await
    }
    fn validate(&self) -> Result<(), IntentError> {
        take(&self.take)?;
        Ok(())
    }
}

impl Intent for IncreaseTake {
    fn summary(&self) -> String {
        format!(
            "increase delegate take to {}/65535",
            take_summary(&self.take)
        )
    }
    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        crate::generated::passthrough::increase_take(self, cx).await
    }
    fn validate(&self) -> Result<(), IntentError> {
        take(&self.take)?;
        Ok(())
    }
}

impl Intent for DecreaseTake {
    fn summary(&self) -> String {
        format!(
            "decrease delegate take to {}/65535",
            take_summary(&self.take)
        )
    }
    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        crate::generated::passthrough::decrease_take(self, cx).await
    }
    fn validate(&self) -> Result<(), IntentError> {
        take(&self.take)?;
        Ok(())
    }
}

impl Intent for SetTake {
    fn summary(&self) -> String {
        match take(&self.take) {
            Ok(raw) => format!(
                "set delegate take to {:.2}% ({} as u16)",
                raw.to_f64() * 100.0,
                raw.0
            ),
            Err(_) => format!("set delegate take to {} as u16", self.take),
        }
    }
    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        let target = take(&self.take)?;
        let hotkey = cx.hotkey_address(self.hotkey_ss58.as_ref().map(Ss58Address::account))?;
        let current = cx.view.storage(&st::DELEGATES, (hotkey,)).await?;
        if target == current {
            return Err(IntentError::validation(format!(
                "delegate take is already {}/65535; nothing to change",
                current.0
            )));
        }
        let call = if target < current {
            subtensor_module::decrease_take(hotkey, target)
        } else {
            subtensor_module::increase_take(hotkey, target)
        };
        Ok(Built::new(call.compose(cx.runtime)?))
    }
    fn validate(&self) -> Result<(), IntentError> {
        take(&self.take)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rtao_client::{BlockRef, Client, ClientConfig, DynView, Network, Snapshot};
    use rtao_primitives::{NetUid, Spend};
    use rtao_runtime::{CallBytes, RuntimeHandle};
    use rtao_signer::{KeyView, SignerError};
    use rtao_testkit::{FakeChain, FakeState, rpc::RpcClient};
    use rtao_tx::{DynIntent, IntentInterface, OriginView, SystemClock};

    const OWNER: AccountId = AccountId([7; 32]);
    const HOTKEY: AccountId = AccountId([8; 32]);
    const CHILD: AccountId = AccountId([9; 32]);

    struct PublicKeys;
    impl KeyView for PublicKeys {
        fn hotkey_address(&self) -> Result<AccountId, SignerError> {
            Ok(HOTKEY)
        }
        fn hotkey_public_key(&self) -> Result<[u8; 32], SignerError> {
            Ok(HOTKEY.0)
        }
        fn coldkey_address(&self) -> Result<AccountId, SignerError> {
            Ok(OWNER)
        }
    }

    async fn fixture(current: Option<u16>) -> anyhow::Result<(FakeChain, Client, Snapshot)> {
        let mut state = FakeState::try_default()?;
        if let Some(current) = current {
            state.seed(&st::DELEGATES, (HOTKEY,), PerU16(current))?;
        } else {
            // A metadata-default case needs an explicit map miss: the imported
            // Python fake otherwise supplies a blanket Delegates value.
            let entry = state
                .runtime
                .storage_entry("SubtensorModule", "Delegates")
                .ok_or_else(|| anyhow::anyhow!("Delegates storage missing"))?;
            let key = state
                .runtime
                .storage_key(entry, &[rtao_core::codec::Value::Bytes(HOTKEY.0.to_vec())])?;
            state.seed_encoded(key, None);
        }
        state.seed(&st::DELEGATES, (CHILD,), PerU16(1000))?;
        let chain = FakeChain::from_state(state);
        let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
        config.track_heads = false;
        config.runtime_cache_dir = None;
        config.config_store = None;
        let client = Client::from_rpc(RpcClient::new(chain.clone()), config).await?;
        let snapshot = client.at(BlockRef::Finalized).await?;
        Ok((chain, client, snapshot))
    }

    async fn build<'a>(
        intent: &'a dyn DynIntent,
        snapshot: &'a Snapshot,
        origin: OriginView<'a>,
    ) -> Result<Built, IntentError> {
        intent
            .build_dyn(&BuildCx {
                view: snapshot as &dyn DynView,
                origin,
                runtime: &snapshot.block().runtime,
                clock: &SystemClock,
                spec: intent.spec(),
            })
            .await
    }

    fn decode(call: &CallBytes, runtime: &RuntimeHandle) -> anyhow::Result<Value> {
        let value = runtime
            .core()
            .decode_spec(&rtao_core::runtime::TypeSpec::Call, call.as_bytes(), true)
            .map_err(rtao_core::CoreError::from)?;
        Ok(rtao_core::codec::value::to_corpus_json(&value))
    }

    fn raw_call(index: u8, hotkey: AccountId, netuid: Option<NetUid>, suffix: &[u8]) -> Vec<u8> {
        let mut bytes = vec![7, index];
        bytes.extend_from_slice(hotkey.as_bytes());
        if let Some(netuid) = netuid {
            bytes.extend_from_slice(&netuid.get().to_le_bytes());
        }
        bytes.extend_from_slice(suffix);
        bytes
    }

    // pyt: unit/test_intents_table.py::TestProportionInputs::test_take_accepts_fraction_and_raw
    // pyt: unit/test_intents_table.py::TestProportionInputs::test_childkey_take_accepts_fraction
    #[test]
    fn take_inputs_keep_raw_integer_and_fraction_meanings_and_normalize_json() -> anyhow::Result<()>
    {
        for (value, expected) in [
            (json!(0), 0),
            (json!(1), 1),
            (json!(1.0), u16::MAX),
            (json!("1"), 1),
            (json!("1.0"), u16::MAX),
            (json!(u16::MAX), u16::MAX),
            (json!(0.18), 11796),
            (json!("0.18"), 11796),
            (json!(0.09), 5898),
            (json!("0.5"), 32768),
            (json!("1e0"), u16::MAX),
            (json!(" +１_２ "), 12),
            (json!("1e-10000"), 0),
        ] {
            assert_eq!(take_to_u16(&value)?, expected, "{value}");
            let mut canonical = value.clone();
            canonical_take(&mut canonical)?;
            assert_eq!(canonical, json!(expected));
            for op in [
                "increase_take",
                "decrease_take",
                "set_childkey_take",
                "set_take",
            ] {
                let mut args = json!({"take":value});
                if op == "set_childkey_take" {
                    args["netuid"] = json!(3);
                }
                let intent = crate::AnyIntent::from_tool(op, args)?;
                assert_eq!(intent.to_json()["take"], expected, "{op}");
                assert!(intent.to_json()["hotkey_ss58"].is_null(), "{op}");
                assert_eq!(
                    crate::AnyIntent::from_tool(op, intent.to_json())?.to_json(),
                    intent.to_json()
                );
            }
        }
        for (take, summary) in [
            (0, "set delegate take to 0.00% (0 as u16)"),
            (11796, "set delegate take to 18.00% (11796 as u16)"),
            (32768, "set delegate take to 50.00% (32768 as u16)"),
            (65535, "set delegate take to 100.00% (65535 as u16)"),
        ] {
            assert_eq!(
                SetTake {
                    take: json!(take),
                    hotkey_ss58: None
                }
                .summary(),
                summary
            );
        }
        Ok(())
    }

    // pyt: unit/test_intents_table.py::TestProportionInputs::test_take_rejects_out_of_range
    #[test]
    fn invalid_takes_refuse_with_python_numeric_diagnostics() -> anyhow::Result<()> {
        for bad in [
            json!(true),
            json!(false),
            json!(65536),
            json!(-1),
            json!(1.5),
            json!("abc"),
            json!(null),
            json!([]),
            json!({}),
            serde_json::from_str::<Value>("1e400")?,
        ] {
            assert!(take_to_u16(&bad).is_err(), "{bad}");
            for op in [
                "increase_take",
                "decrease_take",
                "set_childkey_take",
                "set_take",
            ] {
                let mut args = json!({"take":bad});
                if op == "set_childkey_take" {
                    args["netuid"] = json!(3);
                }
                assert!(crate::AnyIntent::from_tool(op, args).is_err(), "{op}");
            }
        }
        assert_eq!(
            take_to_u16(&json!(true)).err().map(|e| e.to_string()),
            Some("take takes a 0..1 fraction or a raw integer, not a boolean".into())
        );
        assert_eq!(take_to_u16(&json!(-1)).err().map(|e|e.to_string()),
            Some("a raw take must be within 0..65535 (= 1.0); got -1. Pass a value with a decimal point for the 0..1 human form.".into()));
        Ok(())
    }

    // pyt: unit/test_intents_table.py::TestProportionInputs::test_children_accept_fraction_proportions
    // pyt: unit/test_intents_table.py::TestProportionInputs::test_children_accept_raw_shares
    // pyt: unit/test_intents_table.py::TestProportionInputs::test_children_reject_oversum
    // pyt: unit/test_intents_table.py::TestProportionInputs::test_children_roundtrip_after_normalization
    #[test]
    fn child_shares_keep_native_boundaries_python_rounding_and_input_order() -> anyhow::Result<()> {
        let spelling = CHILD.to_ss58(110);
        let mut values = vec![json!([0.5, spelling]), json!([0.25, HOTKEY.to_string()])];
        assert_eq!(
            children(&values)?,
            vec![(1u64 << 63, CHILD), (1u64 << 62, HOTKEY)]
        );
        canonical_children(&mut values)?;
        assert_eq!(
            values,
            vec![
                json!([1u64 << 63, spelling]),
                json!([1u64 << 62, HOTKEY.to_string()])
            ]
        );
        let intent =
            crate::AnyIntent::from_tool("set_children", json!({"netuid":3,"children":values}))?;
        assert_eq!(intent.to_json()["children"], json!(values));
        assert!(intent.to_json()["hotkey_ss58"].is_null());
        assert_eq!(
            crate::AnyIntent::from_tool("set_children", intent.to_json())?.to_json(),
            intent.to_json()
        );
        assert_eq!(
            children(&[json!([u64::MAX, spelling])])?,
            vec![(u64::MAX, CHILD)]
        );
        assert!(children(&[])?.is_empty());
        let repeated = vec![json!([0, spelling]); 6];
        assert_eq!(children(&repeated)?, vec![(0, CHILD); 6]);
        for values in [
            vec![json!([1.0, spelling])],
            vec![json!([0.5, spelling]), json!([0.5, spelling])],
            vec![json!([u64::MAX, spelling]), json!([1, spelling])],
        ] {
            assert_eq!(children(&values).err().map(|error|error.to_string()),
                Some("child proportions sum to 1 of the parent's stake weight; together they must not exceed 1.0".into()));
        }
        for bad in [
            json!(null),
            json!(17),
            json!([]),
            json!([1]),
            json!([1, spelling, 2]),
            json!([true, spelling]),
            json!([-1, spelling]),
            json!(["18446744073709551616", spelling]),
            json!([0, "invalid child"]),
        ] {
            assert!(children(&[bad]).is_err());
        }
        assert_eq!(
            children(&[json!([1])]).err().map(|error| error.to_string()),
            Some("each child entry must be a [proportion, hotkey_ss58] pair; got [1]".into())
        );
        let mut deferred = vec![json!([0.5, "invalid child"])];
        canonical_children(&mut deferred)?;
        let deferred = SetChildren {
            netuid: NetUid(3),
            children: deferred,
            hotkey_ss58: None,
        };
        deferred.validate()?;
        assert_eq!(
            deferred.to_json()["children"],
            json!([[1u64 << 63, "invalid child"]])
        );
        assert!(children(&deferred.children).is_err());
        Ok(())
    }

    #[test]
    fn child_constructor_unpacks_strings_and_object_keys_before_account_validation()
    -> anyhow::Result<()> {
        let address = CHILD.to_ss58(110);
        let mut object = serde_json::Map::new();
        object.insert("0.5".into(), json!({"ignored": [false, null]}));
        object.insert(address.clone(), json!({"also_ignored": -1}));
        let mut values = vec![
            json!("1a"),
            json!("１🦀"),
            Value::Object(object),
            json!([0, {"deferred": "not an account"}]),
        ];
        let expected = json!([
            [1, "a"],
            [1, "🦀"],
            [1u64 << 63, address],
            [0, {"deferred": "not an account"}]
        ]);
        let typed = SetChildren {
            netuid: NetUid(3),
            children: values.clone(),
            hotkey_ss58: None,
        };
        typed.validate()?;
        assert_eq!(typed.to_json()["children"], expected);
        let imported =
            crate::AnyIntent::from_tool("set_children", json!({"netuid":3,"children":values}))?;
        assert_eq!(imported.to_json()["children"], expected);
        assert_eq!(
            crate::AnyIntent::from_tool("set_children", imported.to_json())?.to_json(),
            imported.to_json()
        );
        canonical_children(&mut values)?;
        assert_eq!(json!(values), expected);
        // The single-character account from the first string is rejected only
        // at call building, after constructor share normalization succeeds.
        assert!(children(&values).is_err());
        let object_only = vec![json!({"0.5": false, address.clone(): null})];
        assert_eq!(children(&object_only)?, vec![(1u64 << 63, CHILD)]);
        // Python iterates dictionaries by insertion order, including when
        // the resulting first key is an invalid proportion. Parsing from text
        // pins the public JSON boundary rather than an already sorted map.
        let forward =
            serde_json::from_str::<Value>(&format!(r#"{{"0.5":false,"{address}":null}}"#))?;
        assert_eq!(children(&[forward.clone()])?, vec![(1u64 << 63, CHILD)]);
        assert_eq!(
            crate::AnyIntent::from_tool("set_children", json!({"netuid":3,"children":[forward]}),)?
                .to_json()["children"],
            json!([[1u64 << 63, address]])
        );
        let reversed =
            serde_json::from_str::<Value>(&format!(r#"{{"{address}":null,"0.5":false}}"#))?;
        let expected = proportion(&json!(address), u64::MAX, "child proportion")
            .err()
            .ok_or_else(|| anyhow::anyhow!("account parsed as a child proportion"))?
            .to_string();
        assert_eq!(
            normalized_proportions(&[reversed.clone()])
                .err()
                .map(|error| error.to_string()),
            Some(expected.clone())
        );
        assert_eq!(
            crate::AnyIntent::from_tool("set_children", json!({"netuid":3,"children":[reversed]}),)
                .err()
                .map(|error| error.to_string()),
            Some(expected)
        );
        for entry in [
            json!(""),
            json!("1"),
            json!("1ab"),
            json!("🦀"),
            json!("1🦀x"),
            json!({}),
            json!({"1": null}),
            json!({"1": null, "a": false, "b": []}),
        ] {
            let error = normalized_proportions(&[entry])
                .err()
                .ok_or_else(|| anyhow::anyhow!("malformed child iterable was accepted"))?;
            assert!(
                error
                    .to_string()
                    .starts_with("each child entry must be a [proportion, hotkey_ss58] pair; got ")
            );
        }
        assert_eq!(
            normalized_proportions(&[json!({"1":null})])
                .err()
                .map(|error| error.to_string()),
            Some(
                "each child entry must be a [proportion, hotkey_ss58] pair; got {'1': None}".into()
            )
        );
        assert_eq!(
            normalized_proportions(&[json!("éa")])
                .err()
                .map(|error| error.to_string()),
            proportion(&json!("é"), u64::MAX, "child proportion")
                .err()
                .map(|error| error.to_string())
        );
        let original = vec![json!("1a"), json!([true, "ignored"])];
        let mut invalid = original.clone();
        assert!(canonical_children(&mut invalid).is_err());
        assert_eq!(invalid, original);
        assert_eq!(
            children(&[json!([1, "invalid account"]), json!([1])])
                .err()
                .map(|error| error.to_string()),
            Some("each child entry must be a [proportion, hotkey_ss58] pair; got [1]".into())
        );
        Ok(())
    }

    #[tokio::test]
    async fn child_accounts_keep_native_hex_inputs_and_json_arrays_are_not_raw_bytes()
    -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture(Some(10000)).await?;
        let runtime = &snapshot.block().runtime;
        let before = chain.requests().len();
        let child = AccountId([0xab; 32]);
        let hex = hex::encode(child.as_bytes());
        let mut suffix = vec![4];
        suffix.extend_from_slice(&7u64.to_le_bytes());
        suffix.extend_from_slice(child.as_bytes());
        let expected = raw_call(67, HOTKEY, Some(NetUid(3)), &suffix);
        for spelling in [
            child.to_string(),
            child.to_ss58(110),
            format!("0x{hex}"),
            format!("0x{}", hex.to_uppercase()),
            format!("0x0x{hex}"),
        ] {
            let native = runtime
                .core()
                .coerce_account_id(&rtao_core::codec::Value::Str(spelling.clone()))
                .map_err(rtao_core::CoreError::from)?;
            assert_eq!(native, child.0);
            let entries = vec![json!([7, spelling])];
            assert_eq!(children(&entries)?, vec![(7, child)]);
            let intent = SetChildren {
                netuid: NetUid(3),
                children: entries.clone(),
                hotkey_ss58: None,
            };
            assert_eq!(intent.to_json()["children"], json!(entries));
            let built = build(&intent, &snapshot, OriginView::Keys(&PublicKeys)).await?;
            assert_eq!(built.call.as_bytes(), expected);
            assert_eq!(CallBytes::decode(&expected, runtime)?, built.call);
        }
        for spelling in [
            format!("0X{hex}"),
            hex.clone(),
            "0x".into(),
            "0x01".into(),
            format!("0x{hex} "),
            format!("0xg{}", &hex[1..]),
        ] {
            let native = runtime
                .core()
                .coerce_account_id(&rtao_core::codec::Value::Str(spelling.clone()))
                .err()
                .ok_or_else(|| anyhow::anyhow!("native codec accepted malformed account"))?;
            assert_eq!(
                children(&[json!([7, spelling])])
                    .err()
                    .map(|error| error.to_string()),
                Some(rtao_core::CoreError::from(native).to_string())
            );
        }
        let native_bytes = rtao_core::codec::Value::Bytes(child.as_bytes().to_vec());
        assert_eq!(
            runtime
                .core()
                .coerce_account_id(&native_bytes)
                .map_err(rtao_core::CoreError::from)?,
            child.0
        );
        let json_list = json!(child.as_bytes());
        assert!(children(&[json!([7, json_list])]).is_err());
        let native_list = rtao_core::codec::Value::List(
            child
                .as_bytes()
                .iter()
                .map(|byte| rtao_core::codec::Value::Int(i128::from(*byte)))
                .collect(),
        );
        assert!(runtime.core().coerce_account_id(&native_list).is_err());
        assert_eq!(chain.requests().len(), before);
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn children_and_directional_takes_encode_independent_native_scale_layouts()
    -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture(Some(10000)).await?;
        let runtime = &snapshot.block().runtime;
        let requests = chain.requests().len();
        let set_children = SetChildren {
            netuid: NetUid(u16::MAX),
            children: vec![json!([u64::MAX, CHILD.to_ss58(110)])],
            hotkey_ss58: None,
        };
        let child_take = SetChildkeyTake {
            netuid: NetUid(3),
            take: json!(0.09),
            hotkey_ss58: None,
        };
        let increase = IncreaseTake {
            take: json!(0),
            hotkey_ss58: None,
        };
        let decrease = DecreaseTake {
            take: json!(65535),
            hotkey_ss58: None,
        };
        let mut child_suffix = vec![4]; // One SCALE tuple, with uncompact u64 share.
        child_suffix.extend_from_slice(&u64::MAX.to_le_bytes());
        child_suffix.extend_from_slice(CHILD.as_bytes());
        for (intent, bytes, name) in [
            (
                &set_children as &dyn DynIntent,
                raw_call(67, HOTKEY, Some(NetUid(u16::MAX)), &child_suffix),
                "set_children",
            ),
            (
                &child_take,
                raw_call(75, HOTKEY, Some(NetUid(3)), &5898u16.to_le_bytes()),
                "set_childkey_take",
            ),
            (
                &increase,
                raw_call(66, HOTKEY, None, &0u16.to_le_bytes()),
                "increase_take",
            ),
            (
                &decrease,
                raw_call(65, HOTKEY, None, &u16::MAX.to_le_bytes()),
                "decrease_take",
            ),
        ] {
            let built = build(intent, &snapshot, OriginView::Keys(&PublicKeys)).await?;
            assert_eq!(built.call.as_bytes(), bytes);
            assert_eq!(CallBytes::decode(&bytes, runtime)?, built.call);
            assert!(built.extras.is_empty());
            let decoded = decode(&built.call, runtime)?;
            assert_eq!(decoded["call_module"], "SubtensorModule");
            assert_eq!(decoded["call_function"], name);
            assert_eq!(decoded["call_args"][0]["name"], "hotkey");
            assert_eq!(decoded["call_args"][0]["value"], HOTKEY.to_string());
            if name == "set_children" {
                assert_eq!(
                    decoded["call_args"][2]["value"],
                    json!([[u64::MAX, CHILD.to_string()]])
                );
            } else {
                let raw = if name == "increase_take" {
                    0
                } else if name == "decrease_take" {
                    65535
                } else {
                    5898
                };
                let index = if name == "set_childkey_take" { 2 } else { 1 };
                assert_eq!(decoded["call_args"][index]["value"], raw);
            }
        }
        let revoke = SetChildren {
            netuid: NetUid::ROOT,
            children: vec![],
            hotkey_ss58: Some(CHILD.into()),
        };
        let built = build(&revoke, &snapshot, OriginView::Account(OWNER)).await?;
        assert_eq!(
            built.call.as_bytes(),
            raw_call(67, CHILD, Some(NetUid::ROOT), &[0])
        );
        assert_eq!(chain.requests().len(), requests);
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn set_take_reads_current_public_hotkey_and_selects_direction_or_refuses_equality()
    -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture(Some(10000)).await?;
        let runtime = &snapshot.block().runtime;
        for (target, index, name) in [
            (0, 65, "decrease_take"),
            (11796, 66, "increase_take"),
            (65535, 66, "increase_take"),
        ] {
            let intent = SetTake {
                take: json!(target),
                hotkey_ss58: None,
            };
            let before = chain.requests().len();
            let built = build(&intent, &snapshot, OriginView::Keys(&PublicKeys)).await?;
            assert_eq!(chain.requests().len(), before + 1);
            assert_eq!(
                built.call.as_bytes(),
                raw_call(index, HOTKEY, None, &(target as u16).to_le_bytes())
            );
            assert_eq!(
                CallBytes::decode(built.call.as_bytes(), runtime)?,
                built.call
            );
            assert_eq!(decode(&built.call, runtime)?["call_function"], name);
        }
        let equal = SetTake {
            take: json!(10000),
            hotkey_ss58: None,
        };
        let error = build(&equal, &snapshot, OriginView::Keys(&PublicKeys))
            .await
            .err()
            .ok_or_else(|| anyhow::anyhow!("equal take unexpectedly built"))?;
        assert_eq!(
            error.to_string(),
            "delegate take is already 10000/65535; nothing to change"
        );
        let explicit = SetTake {
            take: json!(1000),
            hotkey_ss58: Some(CHILD.into()),
        };
        let error = build(&explicit, &snapshot, OriginView::Keys(&PublicKeys))
            .await
            .err()
            .ok_or_else(|| anyhow::anyhow!("explicit hotkey was ignored"))?;
        assert_eq!(
            error.to_string(),
            "delegate take is already 1000/65535; nothing to change"
        );
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        client.close().await;
        let (_, client, snapshot) = fixture(None).await?;
        let default = SetTake {
            take: json!(11796),
            hotkey_ss58: None,
        };
        let error = build(&default, &snapshot, OriginView::Keys(&PublicKeys))
            .await
            .err()
            .ok_or_else(|| anyhow::anyhow!("runtime delegate default was ignored"))?;
        assert_eq!(
            error.to_string(),
            "delegate take is already 11796/65535; nothing to change"
        );
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn all_five_preflights_keep_python_defaults_and_bare_coldkeys_need_explicit_hotkeys()
    -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture(Some(10000)).await?;
        let children = SetChildren {
            netuid: NetUid(3),
            children: vec![],
            hotkey_ss58: None,
        };
        let child_take = SetChildkeyTake {
            netuid: NetUid(3),
            take: json!(0.09),
            hotkey_ss58: None,
        };
        let increase = IncreaseTake {
            take: json!(0.18),
            hotkey_ss58: None,
        };
        let decrease = DecreaseTake {
            take: json!(0),
            hotkey_ss58: None,
        };
        let absolute = SetTake {
            take: json!(0.5),
            hotkey_ss58: None,
        };
        for (intent, summary, touches, warnings) in [
            (
                &children as &dyn DynIntent,
                "set 0 child hotkeys on netuid 3",
                vec![NetUid(3)],
                vec!["delegates a share of this hotkey's stake weight to the children"],
            ),
            (
                &child_take,
                "set childkey take to 5898/65535 on netuid 3",
                vec![NetUid(3)],
                vec![],
            ),
            (
                &increase,
                "increase delegate take to 11796/65535",
                vec![],
                vec![],
            ),
            (
                &decrease,
                "decrease delegate take to 0/65535",
                vec![],
                vec![],
            ),
            (
                &absolute,
                "set delegate take to 50.00% (32768 as u16)",
                vec![],
                vec![],
            ),
        ] {
            intent.validate_dyn()?;
            assert_eq!(intent.summary_dyn(), summary);
            assert_eq!(intent.spend_dyn(), Spend::None);
            assert_eq!(intent.touches_netuids_dyn().as_slice(), touches);
            assert!(!intent.affects_all_subnets_dyn());
            assert_eq!(intent.spec().signer, "coldkey");
            assert_eq!(intent.spec().origin, "signed");
            let built = build(intent, &snapshot, OriginView::Keys(&PublicKeys)).await?;
            let before = chain.requests().len();
            let preview = intent
                .preflight_dyn(&PreflightCx {
                    view: &snapshot as &dyn DynView,
                    runtime: &snapshot.block().runtime,
                    clock: &SystemClock,
                    spec: intent.spec(),
                    client: &client,
                    dispatch_origin: OWNER,
                    fee_payer: CHILD,
                    call: &built.call,
                })
                .await?;
            assert_eq!(preview.effects, vec![summary]);
            assert_eq!(preview.warnings, warnings);
            assert!(preview.blocks.is_empty());
            assert!(preview.required_free.is_none());
            assert!(preview.available_free.is_none());
            assert!(preview.estimated_fee.is_none());
            assert!(preview.facts.is_empty());
            assert_eq!(chain.requests().len(), before);
            let error = build(intent, &snapshot, OriginView::Account(OWNER))
                .await
                .err()
                .ok_or_else(|| anyhow::anyhow!("bare coldkey supplied a hotkey"))?;
            assert_eq!(
                error.to_string(),
                format!(
                    "{}: this signer carries no hotkey to default to; pass the hotkey address explicitly or sign with a Wallet",
                    intent.spec().name
                )
            );
            assert_eq!(chain.requests().len(), before);
        }
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        client.close().await;
        Ok(())
    }
}
