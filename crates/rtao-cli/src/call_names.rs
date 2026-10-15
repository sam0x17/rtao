// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/call_names.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Resolve names only in account-typed raw fields and intent account arguments.

use std::fmt;

use rtao::primitives::pyfmt;
use serde_json::{Map, Value};

use crate::books::{
    MAX_NAME_RESOLUTION_DEPTH, NameResolver, ResolvedAddress, is_bittensor_address,
};
use crate::output::{Output, OutputError};

const SIGNATORY_FIELDS: [&str; 2] = ["other_signatories", "signatories"];
const CALL_SPEC_KEYS: [[&str; 3]; 2] = [
    ["module", "function", "params"],
    ["call_module", "call_function", "call_args"],
];

/// Metadata contract needed for raw call account-name resolution.
pub trait CallSchema {
    /// Whether this `Pallet.function` has a generated or live builder contract.
    fn contains_call(&self, target: &str) -> bool;
    /// Return the metadata identity of a call's named parameter.
    fn parameter_type(&self, target: &str, param: &str) -> Option<&str>;
}

/// Frozen generated descriptors, sharing the exact pinned call field identities.
#[derive(Clone, Copy, Debug, Default)]
pub struct GeneratedCallSchema;

impl CallSchema for GeneratedCallSchema {
    fn contains_call(&self, target: &str) -> bool {
        descriptor(target).is_some()
    }

    fn parameter_type(&self, target: &str, param: &str) -> Option<&str> {
        descriptor(target)?
            .fields
            .iter()
            .find(|field| field.name == param)
            .map(|field| field.type_ident)
    }
}

fn descriptor(target: &str) -> Option<&'static rtao_runtime::CallDescriptor> {
    let (pallet, function) = target.split_once('.')?;
    rtao_runtime::calls::ALL
        .iter()
        .copied()
        .find(|descriptor| descriptor.pallet == pallet && descriptor.name == function)
}

/// A parameter error with the CLI option that supplied the JSON.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CallNameError {
    /// Option spelling, such as `--args` or `--dest-ss58`.
    pub param_hint: String,
    /// Public validation diagnostic and optional did-you-mean suggestion.
    pub message: String,
}

impl fmt::Display for CallNameError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for CallNameError {}

/// Resolve names in `Pallet.function` raw JSON using the pinned schema.
pub fn resolve_builder_params(
    names: &NameResolver<'_>,
    target: &str,
    params: &Map<String, Value>,
    param_hint: &str,
) -> Result<Map<String, Value>, CallNameError> {
    resolve_builder_params_with(
        names,
        &GeneratedCallSchema,
        target,
        params,
        param_hint,
        &mut |_, _| {},
    )
}

/// Resolve raw params with an injected schema and renderer callback.
///
/// The callback receives each successfully resolved name, in traversal order.
/// Raw format-42 ss58 strings do not produce a callback, as in Python.
pub fn resolve_builder_params_with(
    names: &NameResolver<'_>,
    schema: &dyn CallSchema,
    target: &str,
    params: &Map<String, Value>,
    param_hint: &str,
    report: &mut dyn FnMut(&str, &ResolvedAddress),
) -> Result<Map<String, Value>, CallNameError> {
    validate_json_nesting(params, param_hint)?;
    RawResolver {
        names,
        schema,
        report,
    }
    .params(target, params, param_hint)
}

/// Resolve `*_ss58` strings, explicit signer lists and nested intent specs.
pub fn resolve_intent_args(
    names: &NameResolver<'_>,
    args: &Map<String, Value>,
    param_hint: Option<&str>,
) -> Result<Map<String, Value>, CallNameError> {
    resolve_intent_args_with(names, args, param_hint, &mut |_, _| {})
}

/// Resolve intent args and report each resolved name to an output renderer.
pub fn resolve_intent_args_with(
    names: &NameResolver<'_>,
    args: &Map<String, Value>,
    param_hint: Option<&str>,
    report: &mut dyn FnMut(&str, &ResolvedAddress),
) -> Result<Map<String, Value>, CallNameError> {
    let param_hint = param_hint.filter(|hint| !hint.is_empty());
    validate_json_nesting(args, param_hint.unwrap_or("--args"))?;
    IntentResolver { names, report }.args(args, param_hint)
}

// Match call_names.py's successful named lookup side effects in both CLI
// callers. Explicit ss58 strings bypass the resolver callback entirely.
pub(crate) fn report_resolved_account(
    output: &Output,
    param: &str,
    resolved: &ResolvedAddress,
) -> Result<(), OutputError> {
    if let Some(name) = &resolved.name {
        output.name_address(&resolved.address, name)?;
    }
    output.classify_address(&resolved.address, resolved.kind)?;
    output.message(&format!(
        "{param}: resolved {} to {}",
        resolved.source, resolved.address
    ))
}

// Text input already uses serde_json's nesting guard. This iterative check
// also protects programmatically constructed Values before recursive clones
// or traversal. Count containers, including the supplied root argument map.
fn validate_json_nesting(args: &Map<String, Value>, hint: &str) -> Result<(), CallNameError> {
    let mut queue: Vec<_> = args.values().map(|value| (value, 1usize)).collect();
    while let Some((value, parent_depth)) = queue.pop() {
        let nested_depth = parent_depth + 1;
        let children: Box<dyn Iterator<Item = &Value> + '_> = match value {
            Value::Array(items) => Box::new(items.iter()),
            Value::Object(fields) => Box::new(fields.values()),
            _ => continue,
        };
        if nested_depth > MAX_NAME_RESOLUTION_DEPTH {
            return Err(CallNameError {
                param_hint: hint.to_owned(),
                message: format!(
                    "JSON nesting exceeds the name resolution limit ({MAX_NAME_RESOLUTION_DEPTH})"
                ),
            });
        }
        queue.extend(children.map(|value| (value, nested_depth)));
    }
    Ok(())
}

struct RawResolver<'a, 'b> {
    names: &'a NameResolver<'b>,
    schema: &'a dyn CallSchema,
    report: &'a mut dyn FnMut(&str, &ResolvedAddress),
}

impl RawResolver<'_, '_> {
    fn params(
        &mut self,
        target: &str,
        params: &Map<String, Value>,
        hint: &str,
    ) -> Result<Map<String, Value>, CallNameError> {
        params
            .iter()
            .map(|(name, value)| {
                let annotation = self.schema.parameter_type(target, name);
                let value = self.value(name, annotation, value, hint)?;
                Ok((name.clone(), value))
            })
            .collect()
    }

    fn value(
        &mut self,
        param: &str,
        annotation: Option<&str>,
        value: &Value,
        hint: &str,
    ) -> Result<Value, CallNameError> {
        if let Value::String(value) = value
            && matches!(annotation, Some("AccountId32" | "MultiAddress"))
        {
            return resolve_account(self.names, param, value, hint, self.report).map(Value::String);
        }
        if is_nested_call(self.schema, value) {
            return self.nested_call(value, hint);
        }
        if let Value::Array(values) = value
            && !values.is_empty()
        {
            if values
                .iter()
                .all(|value| is_nested_call(self.schema, value))
            {
                return values
                    .iter()
                    .map(|value| self.nested_call(value, hint))
                    .collect::<Result<Vec<_>, _>>()
                    .map(Value::Array);
            }
            if SIGNATORY_FIELDS.contains(&param) && values.iter().all(Value::is_string) {
                return values
                    .iter()
                    .map(|value| {
                        // The all-strings condition above defines this list branch.
                        let Some(value) = value.as_str() else {
                            return Ok(value.clone());
                        };
                        resolve_account(self.names, param, value, hint, self.report)
                            .map(Value::String)
                    })
                    .collect::<Result<Vec<_>, _>>()
                    .map(Value::Array);
            }
        }
        Ok(value.clone())
    }

    fn nested_call(&mut self, value: &Value, hint: &str) -> Result<Value, CallNameError> {
        if let Some(keys) = call_spec_keys(value) {
            let Some(object) = value.as_object() else {
                return Ok(value.clone());
            };
            let empty = Map::new();
            let params = match object.get(keys[2]) {
                None | Some(Value::Null) => &empty,
                Some(Value::Object(params)) => params,
                _ => return Ok(value.clone()),
            };
            let module = json_target_part(object.get(keys[0]));
            let function = json_target_part(object.get(keys[1]));
            let target = format!("{module}.{function}");
            let mut resolved = object.clone();
            resolved.insert(
                keys[2].to_owned(),
                Value::Object(self.params(&target, params, hint)?),
            );
            return Ok(Value::Object(resolved));
        }
        if let Some((module, function, params)) = variant_call(self.schema, value) {
            let target = format!("{module}.{function}");
            let params = self.params(&target, params, hint)?;
            let mut function_map = Map::new();
            function_map.insert(function.to_owned(), Value::Object(params));
            let mut module_map = Map::new();
            module_map.insert(module.to_owned(), Value::Object(function_map));
            return Ok(Value::Object(module_map));
        }
        Ok(value.clone())
    }
}

fn json_target_part(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(value)) => value.clone(),
        Some(value) => value.to_string(),
        None => String::new(),
    }
}

fn call_spec_keys(value: &Value) -> Option<[&'static str; 3]> {
    let object = value.as_object()?;
    CALL_SPEC_KEYS
        .into_iter()
        .find(|keys| object.contains_key(keys[0]) && object.contains_key(keys[1]))
}

fn variant_call<'a>(
    schema: &dyn CallSchema,
    value: &'a Value,
) -> Option<(&'a str, &'a str, &'a Map<String, Value>)> {
    let object = value.as_object().filter(|object| object.len() == 1)?;
    let (module, inner) = object.iter().next()?;
    let functions = inner.as_object().filter(|object| object.len() == 1)?;
    let (function, args) = functions.iter().next()?;
    let params = args.as_object()?;
    schema
        .contains_call(&format!("{module}.{function}"))
        .then_some((module, function, params))
}

fn is_nested_call(schema: &dyn CallSchema, value: &Value) -> bool {
    call_spec_keys(value).is_some() || variant_call(schema, value).is_some()
}

struct IntentResolver<'a, 'b> {
    names: &'a NameResolver<'b>,
    report: &'a mut dyn FnMut(&str, &ResolvedAddress),
}

impl IntentResolver<'_, '_> {
    fn args(
        &mut self,
        args: &Map<String, Value>,
        param_hint: Option<&str>,
    ) -> Result<Map<String, Value>, CallNameError> {
        let mut out = args.clone();
        for (name, value) in args {
            let default_hint = format!("--{}", name.replace('_', "-"));
            let hint = param_hint
                .filter(|hint| !hint.is_empty())
                .unwrap_or(&default_hint);
            let resolved = if let Value::String(value) = value
                && name.ends_with("_ss58")
            {
                Value::String(resolve_account(self.names, name, value, hint, self.report)?)
            } else if SIGNATORY_FIELDS.contains(&name.as_str()) && value.is_array() {
                let mut resolved = Vec::new();
                if let Some(values) = value.as_array() {
                    for item in values {
                        resolved.push(match item {
                            Value::String(value) => Value::String(resolve_account(
                                self.names,
                                name,
                                value,
                                hint,
                                self.report,
                            )?),
                            _ => item.clone(),
                        });
                    }
                }
                Value::Array(resolved)
            } else if is_intent_spec(value) {
                self.spec(value, hint)?
            } else if let Value::Array(values) = value {
                Value::Array(
                    values
                        .iter()
                        .map(|item| {
                            if is_intent_spec(item) {
                                self.spec(item, hint)
                            } else {
                                Ok(item.clone())
                            }
                        })
                        .collect::<Result<Vec<_>, _>>()?,
                )
            } else {
                value.clone()
            };
            out.insert(name.clone(), resolved);
        }
        Ok(out)
    }

    fn spec(&mut self, value: &Value, hint: &str) -> Result<Value, CallNameError> {
        let Some(spec) = value.as_object() else {
            return Ok(value.clone());
        };
        let args = spec
            .iter()
            .filter(|(name, _)| name.as_str() != "op")
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect();
        let mut resolved = spec.clone();
        resolved.extend(self.args(&args, Some(hint))?);
        Ok(Value::Object(resolved))
    }
}

fn is_intent_spec(value: &Value) -> bool {
    value
        .as_object()
        .is_some_and(|object| object.contains_key("op"))
}

fn resolve_account(
    names: &NameResolver<'_>,
    param: &str,
    value: &str,
    hint: &str,
    report: &mut dyn FnMut(&str, &ResolvedAddress),
) -> Result<String, CallNameError> {
    if is_bittensor_address(value) {
        return Ok(value.to_owned());
    }
    match names.resolve_address_ref(param, value) {
        Ok(resolved) => {
            report(param, &resolved);
            Ok(resolved.address)
        }
        Err(_) => {
            let base = format!(
                "{param}: {} is not a valid ss58 address",
                pyfmt::string_repr(value)
            );
            let message = if let Some(candidate) = names.closest_name(value) {
                format!(
                    "{base} — did you mean {} {} ({})?",
                    candidate.source,
                    pyfmt::string_repr(&candidate.name),
                    candidate.address
                )
            } else {
                format!("{base} and matches no address-book, proxy-book, or wallet name")
            };
            Err(CallNameError {
                param_hint: hint.to_owned(),
                message,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::books::{AccountKind, AccountLookup, Books, NameCandidate, ResolveError};
    use serde_json::json;
    use std::error::Error;

    const ALICE: &str = "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY";
    const BOB: &str = "5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty";
    const CHARLIE: &str = "5FLSigC9HGRKVhB9FiEo4Y3koPsNmBmLJbpXg2mp1hXcS59Y";

    struct Lookup;
    impl AccountLookup for Lookup {
        fn coldkey(&self, name: &str) -> Result<String, ResolveError> {
            match name {
                "alice" => Ok(ALICE.into()),
                "bob" => Ok(BOB.into()),
                _ => Err(ResolveError::new("wallet unavailable")),
            }
        }
        fn hotkey(&self, wallet: &str, hotkey: &str) -> Result<String, ResolveError> {
            if wallet == "alice" && hotkey == "worker" {
                Ok(CHARLIE.into())
            } else {
                Err(ResolveError::new("hotkey unavailable"))
            }
        }
        fn candidates(&self) -> Vec<NameCandidate> {
            vec![NameCandidate {
                name: "alice/worker".into(),
                address: CHARLIE.into(),
                source: "hotkey".into(),
            }]
        }
    }

    fn books() -> Books {
        Books::from_values(
            json!([{"name":"contact","address":BOB},{"name":"alice","address":ALICE}]),
            json!([{"name":"proxied","address":CHARLIE}]),
            json!([{"name":"treasury","threshold":2,"signatories":[ALICE,BOB]}]),
        )
    }

    fn object(value: Value) -> Result<Map<String, Value>, Box<dyn Error>> {
        match value {
            Value::Object(value) => Ok(value),
            _ => Err("test fixture must be an object".into()),
        }
    }

    #[test]
    fn raw_scalar_accounts_use_generated_field_types_and_canonical_books()
    -> Result<(), Box<dyn Error>> {
        let books = books();
        let names = NameResolver::new(&books, &Lookup, "alice", "worker");
        let transfer = object(json!({"dest":"contact","value":1,"memo":"alice"}))?;
        assert_eq!(
            resolve_builder_params(&names, "Balances.transfer_keep_alive", &transfer, "--args")?,
            object(json!({"dest":BOB,"value":1,"memo":"alice"}))?
        );
        // Port of Python TestAddressResolution's saved-multisig regression,
        // independently checked against upstream golden multisig[0].
        let swap = object(json!({"new_coldkey":"treasury"}))?;
        assert_eq!(
            resolve_builder_params(
                &names,
                "SubtensorModule.schedule_swap_coldkey",
                &swap,
                "--args"
            )?,
            object(json!({"new_coldkey":"5F3QVbS78a4aTYLiRAD8N3czjqVoNyV42L19CXyhqUMCh4Ch"}))?
        );
        let stake = object(json!({"hotkey":"worker","netuid":1,"amount_staked":7}))?;
        assert_eq!(
            resolve_builder_params(&names, "SubtensorModule.add_stake", &stake, "--args")?
                .get("hotkey"),
            Some(&json!(CHARLIE))
        );
        Ok(())
    }

    #[test]
    fn raw_nested_calls_cover_both_keyed_shapes_and_known_runtime_variants()
    -> Result<(), Box<dyn Error>> {
        let books = books();
        let names = NameResolver::new(&books, &Lookup, "alice", "worker");
        let params = object(json!({"calls":[
            {"module":"Balances","function":"transfer_keep_alive","params":{"dest":"contact","value":3},"keep_extra":true},
            {"call_module":"SubtensorModule","call_function":"schedule_swap_coldkey","call_args":{"new_coldkey":"proxied"}},
            {"Sudo":{"sudo":{"call":{"Balances":{"transfer_keep_alive":{"dest":"alice","value":5}}}}}},
        ]}))?;
        let expected = object(json!({"calls":[
            {"module":"Balances","function":"transfer_keep_alive","params":{"dest":BOB,"value":3},"keep_extra":true},
            {"call_module":"SubtensorModule","call_function":"schedule_swap_coldkey","call_args":{"new_coldkey":CHARLIE}},
            {"Sudo":{"sudo":{"call":{"Balances":{"transfer_keep_alive":{"dest":ALICE,"value":5}}}}}},
        ]}))?;
        assert_eq!(
            resolve_builder_params(&names, "Utility.batch_all", &params, "--args")?,
            expected
        );
        Ok(())
    }

    #[test]
    fn raw_multisig_signer_lists_resolve_without_sorting_or_deduplicating_json()
    -> Result<(), Box<dyn Error>> {
        let books = books();
        let names = NameResolver::new(&books, &Lookup, "alice", "worker");
        let params = object(
            json!({"threshold":2,"other_signatories":["contact","alice","contact"],"call":{"Balances":{"transfer_keep_alive":{"dest":"contact","value":1}}}}),
        )?;
        let resolved = resolve_builder_params(&names, "Multisig.as_multi", &params, "--args")?;
        assert_eq!(
            resolved.get("other_signatories"),
            Some(&json!([BOB, ALICE, BOB]))
        );
        assert_eq!(
            resolved.get("call"),
            Some(&json!({"Balances":{"transfer_keep_alive":{"dest":BOB,"value":1}}}))
        );
        Ok(())
    }

    #[test]
    fn arbitrary_lists_numeric_accounts_and_multiaddress_variants_are_left_for_codec()
    -> Result<(), Box<dyn Error>> {
        let books = books();
        let names = NameResolver::new(&books, &Lookup, "alice", "worker");
        // Python explicitly leaves arbitrary string lists, mixed signatory
        // lists and MultiAddress enum shapes unchanged.
        for (target, params) in [
            (
                "System.remark",
                json!({"remark":[BOB,"ordinary memo text"]}),
            ),
            (
                "Balances.transfer_keep_alive",
                json!({"dest":{"Id":"alice"},"value":1}),
            ),
            (
                "Balances.transfer_keep_alive",
                json!({"dest":{"Index":42},"value":1}),
            ),
            ("Balances.transfer_keep_alive", json!({"dest":42,"value":1})),
            (
                "Multisig.as_multi",
                json!({"other_signatories":["missing",42]}),
            ),
            (
                "SubtensorModule.set_children",
                json!({"children":[[1,"missing"]]}),
            ),
        ] {
            let params = object(params)?;
            assert_eq!(
                resolve_builder_params(&names, target, &params, "--args")?,
                params
            );
        }
        Ok(())
    }

    #[test]
    fn unknown_target_preserves_fields_but_recurses_into_known_keyed_calls()
    -> Result<(), Box<dyn Error>> {
        let books = books();
        let names = NameResolver::new(&books, &Lookup, "alice", "worker");
        let params = object(
            json!({"dest":"missing","data":{"Unknown":{"thing":{"dest":"missing"}}},"child":{"module":"Balances","function":"transfer_keep_alive","params":{"dest":"contact","value":1}}}),
        )?;
        let resolved = resolve_builder_params(&names, "Future.new_call", &params, "--args")?;
        assert_eq!(resolved.get("dest"), Some(&json!("missing")));
        assert_eq!(resolved.get("data"), params.get("data"));
        assert_eq!(
            resolved.get("child"),
            Some(
                &json!({"module":"Balances","function":"transfer_keep_alive","params":{"dest":BOB,"value":1}})
            )
        );
        Ok(())
    }

    #[test]
    fn malformed_nested_params_and_mixed_call_lists_retain_their_original_shapes()
    -> Result<(), Box<dyn Error>> {
        let books = books();
        let names = NameResolver::new(&books, &Lookup, "alice", "worker");
        for params in [
            json!({"call":{"module":"Balances","function":"transfer_keep_alive","params":["missing"]}}),
            json!({"calls":[{"module":"Balances","function":"transfer_keep_alive","params":{"dest":"missing"}},42]}),
            json!({"call":{"Balances":{"transfer_keep_alive":{"dest":"missing"},"other":{}}}}),
            json!({"call":{"module":"Balances","params":{"dest":"missing"}}}),
        ] {
            let params = object(params)?;
            assert_eq!(
                resolve_builder_params(&names, "Sudo.sudo", &params, "--args")?,
                params
            );
        }
        for call in [
            json!({"module":"Unknown","function":"new_call"}),
            json!({"module":"Unknown","function":"new_call","params":null}),
        ] {
            let params = object(json!({"call":call}))?;
            assert_eq!(
                resolve_builder_params(&names, "Sudo.sudo", &params, "--args")?.get("call"),
                Some(&json!({"module":"Unknown","function":"new_call","params":{}}))
            );
        }
        Ok(())
    }

    #[test]
    fn intent_accounts_and_nested_batch_multisig_specs_resolve_recursively()
    -> Result<(), Box<dyn Error>> {
        let books = books();
        let names = NameResolver::new(&books, &Lookup, "alice", "worker");
        let args = object(json!({"intents":[
            {"op":"transfer","dest_ss58":"contact","amount_tao":1.5},
            {"op":"multisig_execute","threshold":2,"other_signatories":["alice","contact",7],"call":{"op":"add_stake","hotkey_ss58":"worker","amount_tao":3,"netuid":1}},
            {"memo":"leave this ordinary object","dest_ss58":"missing"},
            42,
        ]}))?;
        let expected = object(json!({"intents":[
            {"op":"transfer","dest_ss58":BOB,"amount_tao":1.5},
            {"op":"multisig_execute","threshold":2,"other_signatories":[ALICE,BOB,7],"call":{"op":"add_stake","hotkey_ss58":CHARLIE,"amount_tao":3,"netuid":1}},
            {"memo":"leave this ordinary object","dest_ss58":"missing"},
            42,
        ]}))?;
        assert_eq!(resolve_intent_args(&names, &args, None)?, expected);
        Ok(())
    }

    #[test]
    fn intent_resolver_does_not_guess_raw_fields_or_plain_nested_args() -> Result<(), Box<dyn Error>>
    {
        let books = books();
        let names = NameResolver::new(&books, &Lookup, "alice", "worker");
        let args = object(
            json!({"dest_ss58":42,"hotkey_ss58":{"Id":"worker"},"signatories":[],"dest":"missing","inner_args":{"dest_ss58":"missing"},"call":{"Balances":{"transfer_keep_alive":{"dest":"missing","value":1}}},"children":[[1,"missing"]]}),
        )?;
        assert_eq!(resolve_intent_args(&names, &args, None)?, args);
        Ok(())
    }

    #[test]
    fn unresolved_accounts_supply_did_you_mean_and_inherited_option_hints()
    -> Result<(), Box<dyn Error>> {
        let books = books();
        let names = NameResolver::new(&books, &Lookup, "alice", "worker");
        let args = object(json!({"dest_ss58":"alcie"}))?;
        let error = resolve_intent_args(&names, &args, None)
            .err()
            .ok_or("expected unresolved name")?;
        assert_eq!(error.param_hint, "--dest-ss58");
        assert_eq!(
            resolve_intent_args(&names, &args, Some(""))
                .err()
                .map(|error| error.param_hint),
            Some("--dest-ss58".into())
        );
        assert_eq!(
            error.message,
            format!(
                "dest_ss58: 'alcie' is not a valid ss58 address — did you mean address-book entry 'alice' ({ALICE})?"
            )
        );
        let batch = object(json!({"call":{"op":"transfer","dest_ss58":"zzzzzz"}}))?;
        let error = resolve_intent_args(&names, &batch, Some("--call"))
            .err()
            .ok_or("expected unresolved nested name")?;
        assert_eq!(error.param_hint, "--call");
        assert_eq!(
            error.message,
            "dest_ss58: 'zzzzzz' is not a valid ss58 address and matches no address-book, proxy-book, or wallet name"
        );
        let raw = object(
            json!({"call":{"Balances":{"transfer_keep_alive":{"dest":"zzzzzz","value":1}}}}),
        )?;
        let error = resolve_builder_params(&names, "Sudo.sudo", &raw, "--args-file")
            .err()
            .ok_or("expected unresolved raw name")?;
        assert_eq!(error.param_hint, "--args-file");
        Ok(())
    }

    #[test]
    fn renderer_callback_receives_only_resolved_names_and_correct_key_classification()
    -> Result<(), Box<dyn Error>> {
        let books = books();
        let names = NameResolver::new(&books, &Lookup, "alice", "worker");
        let mut records = Vec::new();
        let args =
            object(json!({"dest_ss58":ALICE,"hotkey_ss58":"worker","new_coldkey_ss58":"contact"}))?;
        resolve_intent_args_with(&names, &args, None, &mut |param, address| {
            records.push((param.to_owned(), address.clone()))
        })?;
        assert_eq!(records.len(), 2);
        assert!(records.iter().any(|(param, address)| param == "hotkey_ss58"
            && address.kind == AccountKind::Hotkey
            && address.address == CHARLIE
            && address.source == "hotkey 'worker'"));
        assert!(
            records
                .iter()
                .any(|(param, address)| param == "new_coldkey_ss58"
                    && address.kind == AccountKind::Coldkey
                    && address.address == BOB
                    && address.name.as_deref() == Some("contact"))
        );
        Ok(())
    }

    #[test]
    fn programmatic_json_nesting_refuses_before_clone_or_name_lookup() -> Result<(), Box<dyn Error>>
    {
        let books = books();
        let names = NameResolver::new(&books, &Lookup, "alice", "worker");
        for containers in [MAX_NAME_RESOLUTION_DEPTH - 1, MAX_NAME_RESOLUTION_DEPTH] {
            let mut value = Value::String("opaque data".into());
            for _ in 0..containers {
                value = json!({"ordinary":value});
            }
            let args = object(json!({"ordinary":value}))?;
            let raw = resolve_builder_params(&names, "Unknown.call", &args, "--args-file");
            let intent = resolve_intent_args(&names, &args, Some("--call"));
            if containers < MAX_NAME_RESOLUTION_DEPTH {
                assert_eq!(raw?, args);
                assert_eq!(intent?, args);
            } else {
                let raw = raw.err().ok_or("too-deep raw JSON accepted")?;
                let intent = intent.err().ok_or("too-deep intent JSON accepted")?;
                assert_eq!(raw.param_hint, "--args-file");
                assert_eq!(intent.param_hint, "--call");
                assert_eq!(
                    raw.message,
                    format!(
                        "JSON nesting exceeds the name resolution limit ({MAX_NAME_RESOLUTION_DEPTH})"
                    )
                );
            }
        }
        Ok(())
    }

    #[test]
    fn injected_metadata_schema_controls_resolution_without_field_name_guessing()
    -> Result<(), Box<dyn Error>> {
        struct FutureSchema;
        impl CallSchema for FutureSchema {
            fn contains_call(&self, target: &str) -> bool {
                target == "Future.send"
            }
            fn parameter_type(&self, target: &str, param: &str) -> Option<&str> {
                (target == "Future.send" && param == "receiver").then_some("AccountId32")
            }
        }
        let books = books();
        let names = NameResolver::new(&books, &Lookup, "alice", "worker");
        let params = object(json!({"receiver":"contact","dest":"missing"}))?;
        let resolved = resolve_builder_params_with(
            &names,
            &FutureSchema,
            "Future.send",
            &params,
            "--args",
            &mut |_, _| {},
        )?;
        assert_eq!(resolved.get("receiver"), Some(&json!(BOB)));
        assert_eq!(resolved.get("dest"), Some(&json!("missing")));
        assert_eq!(
            resolve_builder_params(&names, "Future.send", &params, "--args")?,
            params
        );
        Ok(())
    }
}
