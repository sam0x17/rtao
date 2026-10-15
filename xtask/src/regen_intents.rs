//! Catalog-derived intent interfaces and reviewed pass-through call maps.

use super::{emit, format_rust, types::TypeMapper};
use anyhow::{Context, Result, bail, ensure};
use rtao_catalog::IntentSpec;
use rtao_core::ir::MetadataIr;
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};
use toml_edit::DocumentMut;

fn document(root: &Path, name: &str) -> Result<DocumentMut> {
    fs::read_to_string(root.join(name))
        .with_context(|| format!("read {name}"))?
        .parse()
        .with_context(|| format!("parse {name}"))
}

fn group(spec: &IntentSpec) -> Result<String> {
    let provenance: Value =
        serde_json::from_str(spec.fields.first().map_or("{}", |f| f.provenance_json))?;
    if let Some(path) = provenance.get("path").and_then(Value::as_str) {
        return Path::new(path)
            .file_stem()
            .and_then(|s| s.to_str())
            .map(str::to_owned)
            .context("intent group");
    }
    // Fieldless classes still have their authenticated class-variable spans.
    let vars: Value = serde_json::from_str(spec.class_vars_json)?;
    let path = vars["op"]["provenance"]["path"]
        .as_str()
        .context("op provenance path")?;
    Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .map(str::to_owned)
        .context("fieldless intent group")
}

fn stripped(name: &str) -> &str {
    ["_ss58", "_tao", "_alpha", "_rao", "_hex"]
        .iter()
        .find_map(|suffix| name.strip_suffix(suffix))
        .unwrap_or(name)
}

fn passthrough_is_copy(ty: &str) -> bool {
    if let Some(inner) = ty
        .strip_prefix("Option<")
        .and_then(|ty| ty.strip_suffix('>'))
    {
        return passthrough_is_copy(inner);
    }
    matches!(
        ty,
        "bool"
            | "u8"
            | "u16"
            | "u32"
            | "u64"
            | "u128"
            | "rtao_primitives::AccountId"
            | "rtao_primitives::NetUid"
            | "rtao_primitives::MechId"
            | "rtao_primitives::Uid"
            | "rtao_primitives::Percent"
    )
}

fn integer(
    spec: &IntentSpec,
    name: &str,
    ir: &MetadataIr,
    mapper: &TypeMapper<'_>,
) -> Result<String> {
    let mut found = BTreeSet::new();
    for (pallet, call) in spec.wraps {
        let call = ir
            .pallets
            .iter()
            .find(|p| p.name == *pallet)
            .and_then(|p| p.calls.iter().find(|c| c.name == *call));
        if let Some(call) = call {
            for field in &call.args {
                if field.name == name || field.name == stripped(name) {
                    let mapped = mapper.argument(pallet, &call.name, field)?;
                    let mapped = mapped
                        .strip_prefix("Option<")
                        .and_then(|ty| ty.strip_suffix('>'))
                        .unwrap_or(&mapped)
                        .to_owned();
                    if matches!(mapped.as_str(), "u8" | "u16" | "u32" | "u64" | "u128") {
                        found.insert(mapped);
                    }
                }
            }
        }
    }
    let rank = |ty: &str| match ty {
        "u8" => 8,
        "u16" => 16,
        "u32" => 32,
        "u64" => 64,
        _ => 128,
    };
    Ok(found
        .into_iter()
        .min_by_key(|ty| rank(ty))
        .unwrap_or_else(|| "u64".into()))
}

fn field_type(
    spec: &IntentSpec,
    field: &rtao_catalog::ParamSpec,
    config: &DocumentMut,
    ir: &MetadataIr,
    mapper: &TypeMapper<'_>,
) -> Result<String> {
    let key = format!("{}.{}", spec.name, field.name);
    if let Some(ty) = config
        .get("fields")
        .and_then(|v| v.get(&key))
        .and_then(|v| v.as_str())
    {
        return Ok(ty.to_owned());
    }
    let annotation = field.annotation.context("intent field annotation")?;
    let optional = annotation.starts_with("Optional[") && annotation.ends_with(']');
    let annotation = if optional {
        &annotation[9..annotation.len() - 1]
    } else {
        annotation
    };
    let name = field.name;
    let ty = if name.ends_with("_ss58") {
        config
            .get("address_types")
            .and_then(|types| types.get("scalar"))
            .and_then(|ty| ty.as_str())
            .context("scalar SS58 input type missing from intent_types.toml")?
            .to_owned()
    } else if name.ends_with("_ss58s") {
        config
            .get("address_types")
            .and_then(|types| types.get("plural"))
            .and_then(|ty| ty.as_str())
            .context("plural SS58 input type missing from intent_types.toml")?
            .to_owned()
    } else if name == "netuid" || name.ends_with("_netuid") {
        "rtao_primitives::NetUid".to_owned()
    } else if name == "mechid" {
        "rtao_primitives::MechId".to_owned()
    } else if name == "uids" {
        "Vec<rtao_primitives::Uid>".to_owned()
    } else if annotation == "Money" {
        if !name.ends_with("_tao") && !name.ends_with("_alpha") {
            ensure!(
                config
                    .get("money_units")
                    .and_then(|v| v.get(&key))
                    .and_then(|v| v.as_str())
                    .is_some(),
                "money property {key} requires an explicit unit source"
            );
        }
        "rtao_primitives::Amount".to_owned()
    } else {
        match annotation {
            "bool" => "bool".into(),
            "str" => "String".into(),
            "int" => integer(spec, name, ir, mapper)?,
            "float" => "rtao_primitives::Ratio".into(),
            "list[int]" if name == "netuids" || name == "subnets" => {
                "Vec<rtao_primitives::NetUid>".into()
            }
            "list[int]" => "Vec<u64>".into(),
            "list[float]" => "Vec<rtao_primitives::Ratio>".into(),
            "list" | "list[dict]" => "Vec<serde_json::Value>".into(),
            "dict" | "int | float | str" => "serde_json::Value".into(),
            _ => bail!("unmapped intent annotation {key}: {annotation}"),
        }
    };
    Ok(if optional {
        format!("Option<{ty}>")
    } else {
        ty
    })
}

fn default_expr(ty: &str, value: &Value) -> Result<String> {
    if value.is_null() {
        ensure!(
            ty.starts_with("Option<"),
            "null default for nonoptional {ty}"
        );
        return Ok("None".into());
    }
    if ty.starts_with("Option<") {
        return Ok(format!(
            "Some({})",
            default_expr(&ty[7..ty.len() - 1], value)?
        ));
    }
    if ty == "String" {
        return Ok(format!(
            "{:?}.to_owned()",
            value.as_str().context("string default")?
        ));
    }
    if ty == "rtao_primitives::Ratio" {
        return Ok(format!(
            "rtao_primitives::Ratio::new({:?}).expect(\"authenticated finite ratio default\")",
            value.as_f64().context("ratio default")?
        ));
    }
    if ty == "rtao_primitives::Amount" {
        return Ok(format!(
            "rtao_primitives::Amount::parse({:?}).expect(\"authenticated native amount default\")",
            value.to_string()
        ));
    }
    if ty == "serde_json::Number" {
        ensure!(value.is_number(), "number default");
        return Ok(format!(
            "{:?}.parse().expect(\"authenticated numeric default\")",
            value.to_string()
        ));
    }
    if ty.starts_with("Vec<") {
        let inner = &ty[4..ty.len() - 1];
        let values = value
            .as_array()
            .context("array default")?
            .iter()
            .map(|v| default_expr(inner, v))
            .collect::<Result<Vec<_>>>()?;
        return Ok(format!("vec![{}]", values.join(",")));
    }
    if ty.starts_with("rtao_primitives::") {
        return Ok(format!("{ty}({value})"));
    }
    Ok(value.to_string())
}

fn outputs(config: &DocumentMut, name: &str) -> String {
    config
        .get("outputs")
        .and_then(|v| v.get(name))
        .and_then(|v| v.as_str())
        .unwrap_or("()")
        .into()
}

fn cfg(feature: &str) -> String {
    format!("#[cfg(feature = {feature:?})]\n")
}

/// Generate all interfaces, dynamic dispatch, defaults and reviewable call maps.
pub(super) fn run(
    root: &Path,
    ir: &MetadataIr,
    mapper: &TypeMapper<'_>,
    check: bool,
) -> Result<()> {
    let config = document(root, "codegen/intent_types.toml")?;
    let callmap = document(root, "codegen/intent_callmap.toml")?;
    let pins = document(root, "upstream.toml")?;
    let commit = pins
        .get("parity")
        .and_then(|v| v.get("commit"))
        .and_then(|v| v.as_str())
        .context("parity commit")?;
    let pending = document(root, "parity/pending.toml")?;
    let pending: BTreeSet<_> = pending
        .get("intents")
        .and_then(|v| v.as_array())
        .context("pending intents array")?
        .iter()
        .map(|v| v.as_str().map(str::to_owned).context("pending op string"))
        .collect::<Result<_>>()?;
    let known: BTreeSet<_> = rtao_catalog::INTENTS
        .iter()
        .map(|s| s.name.to_owned())
        .collect();
    ensure!(pending.is_subset(&known), "unknown pending intent");
    for (name, _) in callmap
        .get("passthrough")
        .and_then(|v| v.as_table())
        .context("pass-through table")?
        .iter()
    {
        ensure!(known.contains(name), "unknown pass-through intent: {name}");
    }
    let source_fields: BTreeSet<_> = rtao_catalog::INTENTS
        .iter()
        .flat_map(|spec| {
            spec.fields
                .iter()
                .map(move |field| format!("{}.{}", spec.name, field.name))
        })
        .collect();
    for table in ["fields", "money_units"] {
        for (name, _) in config
            .get(table)
            .and_then(|v| v.as_table())
            .context("intent type table")?
            .iter()
        {
            ensure!(
                source_fields.contains(name),
                "unknown {table} override: {name}"
            );
        }
    }
    let mut interfaces = String::from("//! Complete catalog-derived intent interfaces.\n");
    let mut defaults = String::from(
        "//! Exact authenticated Python constructor defaults.\n#![allow(clippy::expect_used)]\n",
    );
    let mut stubs = String::from("//! Explicitly pending behavior implementations.\n");
    let mut any = String::from(
        "//! Feature-gated dynamic intent dispatch.\n/// A transaction intent selected from the pinned catalog.\n#[derive(Debug,Clone,PartialEq)]\n#[non_exhaustive]\npub enum AnyIntent {\n",
    );
    let mut groups = BTreeMap::<String, Vec<String>>::new();
    let mut passthrough = String::from("//! Generated builds for reviewed pass-through intents.\n");
    let mut metadata = String::from(
        "//! Interface capabilities derived from the actual feature and behavior inputs.\n\
/// Availability and compiled Rust type information for one catalog intent.\n\
#[derive(Clone,Copy,Debug,PartialEq,Eq)]\npub struct InterfaceMetadata {\n\
/// Authenticated operation name.\npub name:&'static str,\n\
/// Cargo feature containing the interface.\npub feature:&'static str,\n\
/// Whether this binary includes the interface's feature.\npub compiled:bool,\n\
/// Whether the generated build uses implemented behavior instead of a pending stub.\npub implemented:bool,\n\
/// Actual compiled interface type, absent when its feature is disabled.\npub type_name:Option<&'static str>,\n\
/// Actual IntentInterface signing-role associated type.\npub role:Option<&'static str>,\n\
/// Actual IntentInterface result associated type.\npub output:Option<&'static str>,\n\
/// Fields whose configured native type requires a JSON number.\npub json_number_fields:&'static [&'static str],\n}\n\
/// Complete catalog metadata; only compiled and implemented rows can run commands.\n\
pub fn interface_metadata()->Vec<InterfaceMetadata> {\nvec![\n",
    );
    for (index, spec) in rtao_catalog::INTENTS.iter().enumerate() {
        let feature = group(spec)?;
        groups
            .entry(feature.clone())
            .or_default()
            .push(spec.python_class.to_owned());
        let enabled = cfg(&feature);
        let class = spec.python_class;
        interfaces.push_str(&format!("{enabled}#[doc = {:?}]\n#[doc(alias = {:?})]\n#[derive(Debug,Clone,PartialEq,serde::Serialize,serde::Deserialize)]\n#[serde(deny_unknown_fields)]\npub struct {class} {{\n", spec.description, format!("bittensor.intents.{feature}.{class}")));
        defaults.push_str(&format!(
            "{enabled}/// Constructor defaults for `{}`.\npub mod {} {{\n",
            spec.name, spec.name
        ));
        let schema: Value = serde_json::from_str(spec.catalog_json)?;
        let properties = schema["input_schema"]["properties"]
            .as_object()
            .context("intent schema properties")?;
        let source_fields: BTreeSet<_> = spec.fields.iter().map(|f| f.name).collect();
        ensure!(
            properties
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>()
                == source_fields,
            "catalog/source field mismatch: {}",
            spec.name
        );
        let required: BTreeSet<_> = schema["input_schema"]["required"]
            .as_array()
            .map(|a| a.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        ensure!(
            spec.fields
                .iter()
                .filter(|f| f.required)
                .map(|f| f.name)
                .collect::<BTreeSet<_>>()
                == required,
            "catalog/source required field mismatch: {}",
            spec.name
        );
        let mut field_types = BTreeMap::new();
        for field in spec.fields {
            let ty = field_type(spec, field, &config, ir, mapper)?;
            field_types.insert(field.name, ty.clone());
            let name = emit::ident(field.name);
            let doc = schema["input_schema"]["properties"][field.name]["description"]
                .as_str()
                .unwrap_or(field.name)
                // These catalog placeholders are prose, not HTML elements.
                .replace("<intent name>", "&lt;intent name&gt;")
                .replace("<name>", "&lt;name&gt;");
            interfaces.push_str(&format!("#[doc = {doc:?}]\n"));
            if let Some(default) = field.default_json {
                let parsed: Value = serde_json::from_str(default)?;
                let expression = default_expr(&ty, &parsed)?;
                interfaces.push_str(&format!(
                    "#[serde(default = {:?})]\n",
                    format!("super::defaults::{}.{}", spec.name, field.name).replace('.', "::")
                ));
                defaults.push_str(&format!(
                    "/// Extracted default for `{}`.\npub fn {name}() -> {ty} {{ {expression} }}\n",
                    field.name
                ));
            }
            interfaces.push_str(&format!("pub {name}: {ty},\n"));
        }
        interfaces.push_str("}\n");
        defaults.push_str("}\n");
        let implemented = !pending.contains(spec.name);
        let number_fields = spec
            .fields
            .iter()
            .filter(|field| field_types[field.name] == "serde_json::Number")
            .map(|field| format!("{:?}", field.name))
            .collect::<Vec<_>>()
            .join(",");
        metadata.push_str(&format!(
            "{enabled}InterfaceMetadata{{name:{:?},feature:{feature:?},compiled:true,implemented:{implemented},type_name:Some(std::any::type_name::<super::interfaces::{class}>()),role:Some(std::any::type_name::<<super::interfaces::{class} as rtao_tx::IntentInterface>::Role>()),output:Some(std::any::type_name::<<super::interfaces::{class} as rtao_tx::IntentInterface>::Output>()),json_number_fields:&[{number_fields}]}},\n#[cfg(not(feature={feature:?}))]\nInterfaceMetadata{{name:{:?},feature:{feature:?},compiled:false,implemented:{implemented},type_name:None,role:None,output:None,json_number_fields:&[{number_fields}]}},\n",
            spec.name, spec.name,
        ));
        let role = match spec.signer {
            "coldkey" => "Coldkey",
            "hotkey" => "Hotkey",
            _ => bail!("unknown signer"),
        };
        let origin = match spec.origin {
            "signed" => "Signed",
            "root" => "Root",
            "subnet_owner" => "SubnetOwner",
            _ => bail!("unknown origin"),
        };
        let wraps = spec
            .wraps
            .iter()
            .map(|(p, c)| {
                format!(
                    "&rtao_runtime::calls::{}::{}",
                    emit::snake(p),
                    c.to_ascii_uppercase()
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        let json = if matches!(spec.name, "set_weights" | "commit_weights") {
            format!(
                "crate::arguments::weights_json({:?},self,self.uids.as_deref(),self.weights.as_ref())",
                spec.name
            )
        } else if spec.name == "set_hyperparameter" {
            "crate::hyperparameters::intent_json(self)".to_owned()
        } else if matches!(
            spec.name,
            "increase_take" | "decrease_take" | "set_childkey_take" | "set_take"
        ) {
            format!(
                "crate::children::take_json({:?},self,&self.take)",
                spec.name
            )
        } else if spec.name == "set_children" {
            "crate::children::children_json(self)".to_owned()
        } else if spec.name == "swap_basket_many" {
            "crate::staking_baskets::many_json(self)".to_owned()
        } else if spec.name == "fund_evm_key" {
            "crate::evm::fund_json(self)".to_owned()
        } else if spec.name == "batch" {
            "crate::batch::to_json(self)".to_owned()
        } else if spec.name.starts_with("multisig_") {
            let timepoint = match spec.name {
                "multisig_threshold_1" => "None",
                "multisig_cancel" => "Some(&self.timepoint)",
                _ => "self.timepoint.as_ref()",
            };
            format!(
                "crate::multisig::intent_json({:?},self,&self.other_signatories,&self.call,{timepoint})",
                spec.name
            )
        } else {
            format!("crate::args_with_op({:?},self)", spec.name)
        };
        interfaces.push_str(&format!("{enabled}impl rtao_tx::IntentInterface for {class} {{\nconst SPEC: &'static rtao_catalog::IntentSpec = &rtao_catalog::INTENTS[{index}];\ntype Role = rtao_signer::{role};\ntype Origin = rtao_tx::{origin};\ntype Output = {};\nconst WRAPS: &'static [&'static rtao_runtime::CallDescriptor] = &[{wraps}];\nfn to_json(&self) -> serde_json::Value {{ {json} }}\n}}\n", outputs(&config, spec.name)));
        any.push_str(&format!(
            "{enabled}#[doc = {:?}]\n{class}(super::interfaces::{class}),\n",
            spec.summary
        ));
        if pending.contains(spec.name) {
            stubs.push_str(&format!("{enabled}impl rtao_tx::Intent for super::interfaces::{class} {{\nfn summary(&self) -> String {{ <Self as rtao_tx::IntentInterface>::SPEC.summary.to_owned() }}\nasync fn build<V: rtao_client::View + ?Sized>(&self, _cx: &rtao_tx::BuildCx<'_,V>) -> Result<rtao_tx::Built,rtao_tx::IntentError> {{ Err(rtao_tx::IntentError::NotYetPorted({:?})) }}\nfn semantic_intent(&self)-> &dyn rtao_tx::DynIntent {{self}}\n}}\n", spec.name));
        }
        if let Some(mapping) = callmap.get("passthrough").and_then(|v| v.get(spec.name)) {
            let pallet = mapping
                .get("pallet")
                .and_then(|v| v.as_str())
                .context("pass-through pallet")?;
            let call = mapping
                .get("call")
                .and_then(|v| v.as_str())
                .context("pass-through call")?;
            ensure!(
                spec.wraps.contains(&(pallet, call)),
                "pass-through call must be in WRAPS: {}",
                spec.name
            );
            let call_ir = ir
                .pallets
                .iter()
                .find(|p| p.name == pallet)
                .and_then(|p| p.calls.iter().find(|c| c.name == call))
                .context("mapped call missing in metadata")?;
            let mut args = Vec::new();
            for argument in &call_ir.args {
                let field = mapping
                    .get("args")
                    .and_then(|v| v.get(&argument.name))
                    .and_then(|v| v.as_str())
                    .map(str::to_owned)
                    .unwrap_or_else(|| {
                        spec.fields
                            .iter()
                            .find(|f| stripped(f.name) == argument.name)
                            .map_or_else(|| argument.name.clone(), |f| f.name.to_owned())
                    });
                ensure!(
                    field_types.contains_key(field.as_str()),
                    "{} call argument {} has no field {field}",
                    spec.name,
                    argument.name
                );
                let source_ty = &field_types[field.as_str()];
                let target_ty = mapper.argument(pallet, call, argument)?;
                let access = format!("intent.{}", emit::ident(&field));
                let expr = if target_ty == *source_ty {
                    if passthrough_is_copy(&target_ty) {
                        access.clone()
                    } else {
                        format!("{access}.clone()")
                    }
                } else if source_ty == "rtao_primitives::Ss58Address"
                    && target_ty == "rtao_primitives::AccountId"
                {
                    format!("{access}.account()")
                } else if source_ty == "rtao_primitives::Ss58Address"
                    && target_ty == "rtao_core::DynValue"
                {
                    format!("crate::account_argument({access}.account())")
                } else if source_ty == "Option<rtao_primitives::Ss58Address>"
                    && target_ty == "rtao_primitives::AccountId"
                {
                    format!(
                        "cx.hotkey_address({access}.as_ref().map(rtao_primitives::Ss58Address::account))?"
                    )
                } else if source_ty == "Option<rtao_primitives::Ss58Address>"
                    && target_ty == "Option<rtao_primitives::AccountId>"
                {
                    format!("{access}.as_ref().map(rtao_primitives::Ss58Address::account)")
                } else if source_ty == "Vec<rtao_primitives::Ss58Address>"
                    && target_ty == "Vec<rtao_primitives::AccountId>"
                {
                    format!("{access}.iter().map(rtao_primitives::Ss58Address::account).collect()")
                } else if source_ty == "String" && target_ty == "Vec<u8>" {
                    format!("{access}.as_bytes().to_vec()")
                } else if source_ty == "String" && target_ty == "rtao_core::DynValue" {
                    format!(
                        "rtao_core::DynValue::new(rtao_core::codec::Value::Str({access}.clone()))"
                    )
                } else if source_ty == "Vec<rtao_primitives::NetUid>"
                    && target_ty == "rtao_core::DynValue"
                {
                    format!(
                        "rtao_core::DynValue::new(rtao_primitives::IntoCoreValue::into_core_value({access}.clone()))"
                    )
                } else if source_ty == "String" && target_ty.starts_with("[u8;") {
                    format!("crate::arguments::hex_fixed(&{access})?")
                } else if source_ty == "String" && target_ty == "crate::models::ProxyType" {
                    format!("serde_json::from_value(serde_json::Value::String({access}.clone()))?")
                } else if source_ty == "serde_json::Value" && target_ty == "rtao_primitives::PerU16"
                {
                    format!("crate::arguments::take(&{access})?")
                } else if source_ty == "rtao_primitives::Amount"
                    && (target_ty == "rtao_primitives::TaoBalance"
                        || target_ty == "rtao_primitives::AlphaBalance"
                        || target_ty == "u64")
                {
                    let resolver = mapping
                        .get("resolvers")
                        .and_then(|v| v.get(&field))
                        .and_then(|v| v.as_str());
                    if let Some(resolver) = resolver {
                        format!("crate::{}::{resolver}(intent,cx).await?", feature)
                    } else if target_ty == "rtao_primitives::TaoBalance" {
                        format!("crate::arguments::tao_amount(&{access})?.to_wire()")
                    } else if target_ty == "rtao_primitives::AlphaBalance" {
                        format!("crate::arguments::alpha_amount(&{access},intent.netuid)?")
                    } else {
                        format!("crate::arguments::tao_amount(&{access})?.rao()")
                    }
                } else if source_ty == "Option<rtao_primitives::Amount>"
                    && target_ty == "Option<rtao_primitives::TaoBalance>"
                {
                    format!(
                        "{access}.as_ref().map(crate::arguments::tao_amount).transpose()?.map(|amount|amount.to_wire())"
                    )
                } else if source_ty == "Vec<serde_json::Value>" && argument.name == "children" {
                    format!("crate::arguments::children(&{access})?")
                } else if source_ty == "Vec<serde_json::Value>" && argument.name == "legs" {
                    format!("crate::arguments::basket_legs(&{access})?")
                } else {
                    bail!(
                        "unsupported pass-through conversion {}.{field}: {source_ty} -> {target_ty}",
                        spec.name
                    )
                };
                args.push(expr);
            }
            if !pending.contains(spec.name) {
                let intent_arg = if args.iter().any(|argument| argument.contains("intent")) {
                    "intent"
                } else {
                    "_intent"
                };
                passthrough.push_str(&format!("{enabled}pub(crate) async fn {}<V:rtao_client::View + ?Sized>({intent_arg}:&super::interfaces::{class},cx:&rtao_tx::BuildCx<'_,V>)->Result<rtao_tx::Built,rtao_tx::IntentError> {{ let call=rtao_runtime::calls::{}::{}({}); Ok(rtao_tx::Built {{ call:call.compose(cx.runtime)?, extras:serde_json::Map::new() }}) }}\n", spec.name, emit::snake(pallet), emit::ident(call), args.join(",")));
            }
        }
    }
    any.push_str("}\nimpl AnyIntent {\n/// Decode an object of tool arguments, ignoring a top-level `op` exactly as Python does.\npub fn from_tool(op:&str,mut args:serde_json::Value)->Result<Self,rtao_tx::IntentError> {\nif let Some(fields)=args.as_object_mut(){fields.remove(\"op\");}\nmatch op {\n");
    for spec in &rtao_catalog::INTENTS {
        let feature = group(spec)?;
        let class = spec.python_class;
        let construct = if matches!(spec.name, "set_weights" | "commit_weights") {
            format!(
                "let mut intent:super::interfaces::{class}=serde_json::from_value(args)?; crate::arguments::canonical_weights(&mut intent.uids,&mut intent.weights)?;"
            )
        } else if spec.name == "set_hyperparameter" {
            format!(
                "let mut intent:super::interfaces::{class}=serde_json::from_value(args)?; crate::hyperparameters::canonicalize(&mut intent)?;"
            )
        } else if spec.name == "create_crowdloan" {
            format!(
                "crate::crowdloan::check_constructor_input(&args)?; let intent:super::interfaces::{class}=serde_json::from_value(args)?;"
            )
        } else if spec.name == "execute_proxy_announced" {
            format!(
                "crate::proxy::check_constructor_input(&args)?; let intent:super::interfaces::{class}=serde_json::from_value(args)?;"
            )
        } else if spec.name == "batch" {
            format!(
                "crate::batch::check_constructor_input(&args)?; let mut intent:super::interfaces::{class}=serde_json::from_value(args)?; crate::batch::canonicalize(&mut intent)?;"
            )
        } else if spec.name.starts_with("multisig_") {
            format!(
                "crate::multisig::check_constructor_input(&args)?; let intent:super::interfaces::{class}=serde_json::from_value(args)?;"
            )
        } else if matches!(
            spec.name,
            "increase_take" | "decrease_take" | "set_childkey_take" | "set_take"
        ) {
            format!(
                "let mut intent:super::interfaces::{class}=serde_json::from_value(args)?; crate::children::canonical_take(&mut intent.take)?;"
            )
        } else if spec.name == "set_children" {
            format!(
                "let mut intent:super::interfaces::{class}=serde_json::from_value(args)?; crate::children::canonical_children(&mut intent.children)?;"
            )
        } else if spec.name == "swap_basket_many" {
            format!(
                "let mut intent:super::interfaces::{class}=serde_json::from_value(args)?; crate::staking_baskets::canonical_legs(&mut intent.legs)?;"
            )
        } else if spec.name == "fund_evm_key" {
            format!(
                "crate::evm::canonicalize_fund_input(&mut args)?; let intent:super::interfaces::{class}=serde_json::from_value(args)?;"
            )
        } else {
            format!("let intent:super::interfaces::{class}=serde_json::from_value(args)?;")
        };
        any.push_str(&format!("{}{:?} => {{ if !args.is_object() {{return Err(rtao_tx::IntentError::Validation(\"Intent arguments must be a JSON object.\".into()));}} {construct} rtao_tx::Intent::validate(&intent)?; Ok(Self::{class}(intent)) }},\n", cfg(&feature), spec.name));
    }
    any.push_str("_ => Err(crate::registry::unknown_op(op)),\n}}\n");
    for (name, result, body) in [
        (
            "to_json",
            "serde_json::Value",
            "rtao_tx::IntentInterface::to_json(intent)",
        ),
        (
            "spec",
            "&'static rtao_catalog::IntentSpec",
            "<super::interfaces::CLASS as rtao_tx::IntentInterface>::SPEC",
        ),
        ("as_dyn", "&dyn rtao_tx::DynIntent", "intent"),
    ] {
        any.push_str(&format!("/// Return this intent's {name} projection.\npub fn {name}(&self)->{result} {{match self {{\n"));
        for spec in &rtao_catalog::INTENTS {
            any.push_str(&format!(
                "{}Self::{}(_intent)=>{},\n",
                cfg(&group(spec)?),
                spec.python_class,
                body.replace("CLASS", spec.python_class)
                    .replace("intent", "_intent")
            ));
        }
        // With no intent feature this enum is uninhabited; matching by value lets rustc prove it.
        let no_features = groups
            .keys()
            .map(|feature| format!("feature = {feature:?}"))
            .collect::<Vec<_>>()
            .join(",");
        any.push_str(&format!(
            "#[cfg(not(any({no_features})))]\n_ => match *self {{}},\n}}}}\n"
        ));
    }
    any.push_str("/// Own this intent as an object-safe behavior for executor injection.\npub fn into_dyn(self)->Box<dyn rtao_tx::DynIntent> { match self {\n");
    for spec in &rtao_catalog::INTENTS {
        any.push_str(&format!(
            "{}Self::{}(intent)=>Box::new(intent),\n",
            cfg(&group(spec)?),
            spec.python_class
        ));
    }
    any.push_str("}}\n");
    any.push_str("}\n");
    metadata.push_str("]\n}\n");
    let enabled_features = groups
        .keys()
        .map(|feature| format!("feature = {feature:?}"))
        .collect::<Vec<_>>()
        .join(",");
    let mut modules = format!(
        "//! Generated transaction interfaces and dispatch.\nmod interfaces;\nmod any_intent;\npub mod metadata;\npub use metadata::{{InterfaceMetadata,interface_metadata}};\npub(crate) mod defaults;\npub(crate) mod passthrough;\nmod pending;\n#[cfg(any({enabled_features}))]\npub use interfaces::*;\npub use any_intent::AnyIntent;\n"
    );
    for (feature, classes) in groups {
        modules.push_str(&format!("{}/// Intent types in the `{feature}` Python module.\npub mod {feature} {{pub use super::interfaces::{{{}}};}}\n", cfg(&feature), classes.join(",")));
    }
    let files = [
        ("interfaces", interfaces),
        ("defaults", defaults),
        ("pending", stubs),
        ("any_intent", any),
        ("passthrough", passthrough),
        ("metadata", metadata),
        ("mod", modules),
    ];
    let dir = root.join("crates/rtao-intents/src/generated");
    if !check {
        fs::create_dir_all(&dir)?;
    }
    ensure!(
        fs::symlink_metadata(&dir)?.file_type().is_dir(),
        "generated intents directory must be a directory"
    );
    let expected: BTreeSet<_> = files.iter().map(|(name, _)| format!("{name}.rs")).collect();
    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        ensure!(
            expected.contains(&entry.file_name().to_string_lossy().into_owned()),
            "orphan generated intent file: {}",
            entry.path().display()
        );
    }
    let mut differences = Vec::new();
    for (name, body) in files {
        let output = format!(
            "// Derived from RaoFoundation/subtensor sdk/python/bittensor/intents and website catalog/intents.json @ {commit} (Apache-2.0); modified by rtao.\n// @generated by cargo xtask regen; do not edit.\n{}",
            format_rust(&body)?
        );
        let path = dir.join(format!("{name}.rs"));
        if let Ok(metadata) = fs::symlink_metadata(&path) {
            ensure!(
                metadata.file_type().is_file(),
                "generated intent output must be regular: {}",
                path.display()
            );
        }
        if fs::read_to_string(&path).ok().as_deref() != Some(&output) {
            if check {
                differences.push(name);
            } else {
                fs::write(path, output)?;
            }
        }
    }
    ensure!(
        differences.is_empty(),
        "generated intent drift: {}",
        differences.join(", ")
    );
    println!(
        "83 intent interfaces and {} pending behaviors {}",
        pending.len(),
        if check { "match" } else { "regenerated" }
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    fn root() -> &'static Path {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .find(|path| path.join("upstream.toml").is_file())
            .unwrap()
    }
    fn fixture() -> (rtao_core::runtime::Runtime, MetadataIr, DocumentMut) {
        let root = root();
        let runtime = rtao_core::runtime::Runtime::parse(
            &fs::read(root.join("vendor/metadata/finney/478.v15.scale")).unwrap(),
            478,
            1,
            42,
        )
        .unwrap();
        let ir = MetadataIr::from_runtime(&runtime).unwrap();
        let config = document(root, "codegen/intent_types.toml").unwrap();
        (runtime, ir, config)
    }
    fn mapper(runtime: &rtao_core::runtime::Runtime) -> TypeMapper<'_> {
        let root = root();
        TypeMapper::new(
            runtime,
            &document(root, "codegen/types.toml").unwrap(),
            &document(root, "codegen/models.toml").unwrap(),
            &document(root, "codegen/overrides.toml").unwrap(),
        )
        .unwrap()
    }
    #[test]
    fn money_without_a_unit_suffix_requires_an_explicit_currency_source() {
        let (runtime, ir, mut config) = fixture();
        let spec = rtao_catalog::INTENTS
            .iter()
            .find(|spec| spec.name == "swap_basket")
            .unwrap();
        let field = spec
            .fields
            .iter()
            .find(|field| field.name == "amount")
            .unwrap();
        assert_eq!(
            field_type(spec, field, &config, &ir, &mapper(&runtime)).unwrap(),
            "rtao_primitives::Amount"
        );
        config["money_units"]
            .as_table_mut()
            .unwrap()
            .remove("swap_basket.amount");
        assert!(
            field_type(spec, field, &config, &ir, &mapper(&runtime))
                .unwrap_err()
                .to_string()
                .contains("requires an explicit unit source")
        );
    }
    #[test]
    fn inherited_optional_integer_fields_use_the_native_call_width() {
        let (runtime, ir, config) = fixture();
        let spec = rtao_catalog::INTENTS
            .iter()
            .find(|spec| spec.name == "register_leased_network")
            .unwrap();
        let field = spec
            .fields
            .iter()
            .find(|field| field.name == "end_block")
            .unwrap();
        assert_eq!(
            field_type(spec, field, &config, &ir, &mapper(&runtime)).unwrap(),
            "Option<u32>"
        );
        assert_eq!(
            default_expr("rtao_primitives::Ratio", &serde_json::json!(300)).unwrap(),
            "rtao_primitives::Ratio::new(300.0).expect(\"authenticated finite ratio default\")"
        );
        assert!(default_expr("u64", &Value::Null).is_err());
    }
    #[test]
    fn address_arguments_retain_input_spelling_while_wire_accounts_stay_binary() {
        let (runtime, ir, mut config) = fixture();
        let mapper = mapper(&runtime);
        for (name, expected) in [
            ("transfer_all", "rtao_primitives::Ss58Address"),
            ("root_register", "Option<rtao_primitives::Ss58Address>"),
        ] {
            let spec = rtao_catalog::INTENTS
                .iter()
                .find(|spec| spec.name == name)
                .unwrap();
            let field = spec
                .fields
                .iter()
                .find(|field| field.name.ends_with("_ss58"))
                .unwrap();
            assert_eq!(
                field_type(spec, field, &config, &ir, &mapper).unwrap(),
                expected
            );
        }
        let call = ir
            .pallets
            .iter()
            .find(|pallet| pallet.name == "Balances")
            .unwrap()
            .calls
            .iter()
            .find(|call| call.name == "transfer_all")
            .unwrap();
        let destination = call.args.iter().find(|field| field.name == "dest").unwrap();
        assert_eq!(
            mapper
                .argument("Balances", "transfer_all", destination)
                .unwrap(),
            "rtao_core::DynValue"
        );
        let spec = rtao_catalog::INTENTS
            .iter()
            .find(|spec| spec.name == "transfer_all")
            .unwrap();
        let scalar = spec
            .fields
            .iter()
            .find(|field| field.name == "dest_ss58")
            .unwrap();
        let plural = rtao_catalog::ParamSpec {
            name: "dest_ss58s",
            annotation: Some("list[str]"),
            ..*scalar
        };
        assert_eq!(
            field_type(spec, &plural, &config, &ir, &mapper).unwrap(),
            "Vec<rtao_primitives::Ss58Address>"
        );
        config["address_types"]
            .as_table_mut()
            .unwrap()
            .remove("scalar");
        assert!(
            field_type(spec, scalar, &config, &ir, &mapper)
                .unwrap_err()
                .to_string()
                .contains("scalar SS58 input type missing")
        );
        config["address_types"]
            .as_table_mut()
            .unwrap()
            .remove("plural");
        assert!(
            field_type(spec, &plural, &config, &ir, &mapper)
                .unwrap_err()
                .to_string()
                .contains("plural SS58 input type missing")
        );
    }
    #[test]
    fn all_44_reviewed_passthrough_calls_are_distinct_catalog_members() {
        let root = root();
        let config = document(root, "codegen/intent_callmap.toml").unwrap();
        let mappings = config["passthrough"].as_table().unwrap();
        assert_eq!(mappings.len(), 44);
        for (name, call) in mappings.iter() {
            let spec = rtao_catalog::INTENTS
                .iter()
                .find(|spec| spec.name == name)
                .unwrap();
            let pallet = call["pallet"].as_str().unwrap();
            let call = call["call"].as_str().unwrap();
            assert!(spec.wraps.contains(&(pallet, call)), "{name}");
        }
        assert_eq!(
            mappings["transfer_all"]["call"].as_str(),
            Some("transfer_all")
        );
        assert!(mappings.get("multisig_threshold_1").is_none());
    }
}
