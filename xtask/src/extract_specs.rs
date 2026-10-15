//! Authenticated public intent/read/error specs and separately sourced defaults.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use anyhow::{Context, Result, bail, ensure};
use rustpython_parser::{
    Parse,
    ast::{self, Ranged},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use toml_edit::DocumentMut;

use super::{Binding, Resolver, call_path, header, line, pretty, rust_string, rustfmt};

const INTENTS: &str = "vendor/upstream/catalog/intents.json";
const READS: &str = "vendor/upstream/catalog/reads.json";
const VARS: &[&str] = &[
    "op",
    "signer",
    "origin",
    "verify",
    "wraps",
    "all_amount_fields",
    "mev_shield_default",
    "mev_shield_required",
];

fn artifact(root: &Path, pins: &DocumentMut, destination: &str) -> Result<(Value, Value)> {
    let (path, pin) = pins["artifacts"]
        .as_table_like()
        .context("artifact pins missing")?
        .iter()
        .find(|(_, pin)| pin.get("to").and_then(toml_edit::Item::as_str) == Some(destination))
        .with_context(|| format!("catalog artifact {destination} missing"))?;
    let bytes = fs::read(root.join(destination))?;
    let hash = pin
        .get("sha256")
        .and_then(toml_edit::Item::as_str)
        .context("catalog hash missing")?;
    ensure!(
        format!("{:x}", Sha256::digest(&bytes)) == hash,
        "catalog {destination} differs from pinned SHA256"
    );
    let mut data: Value = serde_json::from_slice(&bytes)?;
    data.sort_all_objects();
    Ok((
        data,
        json!({"path":path,"sha256":hash,"tag":pins["parity"]["tag"].as_str(),"commit":pins["parity"]["commit"].as_str()}),
    ))
}

fn span(
    resolver: &Resolver<'_>,
    path: &str,
    source: &str,
    range: rustpython_parser::text_size::TextRange,
) -> Value {
    json!({"path":path,"blob":resolver.tree.get(path),"line":line(source,range.start().to_usize()),"end_line":line(source,range.end().to_usize()),"start_byte":range.start().to_usize(),"end_byte":range.end().to_usize()})
}

struct Class {
    path: String,
    source: String,
    node: ast::StmtClassDef,
    dataclass: bool,
}

fn classes(resolver: &Resolver<'_>) -> Result<BTreeMap<String, Class>> {
    let mut result = BTreeMap::new();
    for path in resolver
        .tree
        .keys()
        .filter(|path| path.starts_with("sdk/python/bittensor/intents/") && path.ends_with(".py"))
    {
        let source = resolver.source(path)?;
        for stmt in ast::Suite::parse(&source, path).map_err(|error| anyhow::anyhow!("{error}"))? {
            if let ast::Stmt::ClassDef(node) = stmt {
                let dataclass = node.decorator_list.iter().any(|expr| match expr {
                    ast::Expr::Call(call) => call_path(&call.func).as_deref() == Some("dataclass"),
                    _ => call_path(expr).as_deref() == Some("dataclass"),
                });
                let name = node.name.to_string();
                ensure!(
                    result
                        .insert(
                            name.clone(),
                            Class {
                                path: path.clone(),
                                source: source.clone(),
                                node,
                                dataclass
                            }
                        )
                        .is_none(),
                    "intent class {name} is ambiguous across source modules"
                );
            }
        }
    }
    Ok(result)
}

fn ancestry<'a>(
    name: &str,
    classes: &'a BTreeMap<String, Class>,
    visiting: &mut BTreeSet<String>,
    out: &mut Vec<&'a Class>,
) -> Result<()> {
    ensure!(
        visiting.insert(name.to_owned()),
        "cyclic intent inheritance at {name}"
    );
    let class = classes
        .get(name)
        .with_context(|| format!("intent base class {name} missing"))?;
    if name != "Intent" {
        ensure!(
            class.node.bases.len() == 1,
            "intent {name} needs one statically resolved base"
        );
        let ast::Expr::Name(base) = &class.node.bases[0] else {
            bail!("intent base must remain a bare class name");
        };
        ancestry(base.id.as_str(), classes, visiting, out)?;
    }
    out.push(class);
    visiting.remove(name);
    Ok(())
}

fn bound_value(resolver: &mut Resolver<'_>, class: &Class, name: &str) -> Result<Value> {
    resolver.module(&class.path)?;
    let key = format!("{}.{}", class.node.name, name);
    match resolver.modules[&class.path].bindings.get(&key) {
        Some(Binding::Value(value)) => value.json(),
        Some(Binding::Refused(reason)) => bail!("{}: {key}: {reason}", class.path),
        _ => bail!("{}: unresolved intent declaration {key}", class.path),
    }
}

fn intent_documents(
    root: &Path,
    pins: &DocumentMut,
    catalog: &Value,
    catalog_provenance: &Value,
) -> Result<(Value, Value)> {
    let mut resolver = Resolver::new(root)?;
    let classes = classes(&resolver)?;
    let rows = catalog
        .as_array()
        .context("intent catalog must be an array")?;
    ensure!(
        rows.len() == 83,
        "intent catalog count changed; review the complete generated surface"
    );
    let mut fields = Vec::new();
    let mut variables = Vec::new();
    let mut names = BTreeSet::new();
    for row in rows {
        let name = row["name"].as_str().context("intent name missing")?;
        let class_name = row["python_class"]
            .as_str()
            .context("intent class missing")?;
        ensure!(names.insert(name), "duplicate intent catalog name {name}");
        let mut ancestors = Vec::new();
        ancestry(class_name, &classes, &mut BTreeSet::new(), &mut ancestors)?;
        let class = *ancestors.last().context("intent ancestry empty")?;
        ensure!(
            class.dataclass
                && class.node.decorator_list.len() == 2
                && class.node.decorator_list.iter().all(|expr| matches!(
                    call_path(expr).as_deref(),
                    Some("register" | "dataclass")
                ))
                && class
                    .node
                    .decorator_list
                    .iter()
                    .any(|expr| call_path(expr).as_deref() == Some("register")),
            "catalog intent must remain a registered dataclass: {class_name}"
        );
        let mut effective = BTreeMap::new();
        let mut effective_fields: Vec<Value> = Vec::new();
        for ancestor in ancestors {
            for stmt in &ancestor.node.body {
                let declaration = match stmt {
                    ast::Stmt::Assign(assign) if assign.targets.len() == 1 => {
                        match &assign.targets[0] {
                            ast::Expr::Name(name) if VARS.contains(&name.id.as_str()) => {
                                Some((name.id.as_str(), None, Some(&*assign.value), true))
                            }
                            _ => None,
                        }
                    }
                    ast::Stmt::AnnAssign(assign) => match &*assign.target {
                        ast::Expr::Name(name) => Some((
                            name.id.as_str(),
                            Some(&*assign.annotation),
                            assign.value.as_deref(),
                            VARS.contains(&name.id.as_str())
                                || matches!(&*assign.annotation, ast::Expr::Subscript(annotation) if call_path(&annotation.value).as_deref() == Some("ClassVar")),
                        )),
                        _ => None,
                    },
                    _ => None,
                };
                let Some((field, annotation, value, class_var)) = declaration else {
                    continue;
                };
                let source = span(&resolver, &ancestor.path, &ancestor.source, stmt.range());
                if class_var {
                    if value.is_some() {
                        effective.insert(field.to_owned(), json!({"value":bound_value(&mut resolver, ancestor, field)?,"provenance":source}));
                    }
                } else if ancestor.dataclass {
                    let raw = if value.is_some() {
                        Some(bound_value(&mut resolver, ancestor, field)?)
                    } else {
                        None
                    };
                    let required = raw
                        .as_ref()
                        .is_none_or(|value| value == &json!({"required":true}));
                    let factory = matches!(value, Some(ast::Expr::Call(call)) if call.keywords.iter().any(|keyword| keyword.arg.as_ref().is_some_and(|arg| arg.as_str() == "default_factory")));
                    let annotation = annotation.map(|expr| {
                        &ancestor.source
                            [expr.range().start().to_usize()..expr.range().end().to_usize()]
                    });
                    let item = json!({"name":field,"annotation":annotation,"required":required,"default":{"present":!required,"factory":factory,"value":if required { Value::Null } else { raw.context("default missing")? }},"provenance":source});
                    if let Some(index) = effective_fields
                        .iter()
                        .position(|row| row["name"].as_str() == Some(field))
                    {
                        effective_fields[index] = item;
                    } else {
                        effective_fields.push(item);
                    }
                }
            }
        }
        for property in ["op", "signer", "origin", "verify"] {
            let expected = if property == "op" {
                &row["name"]
            } else {
                &row[property]
            };
            ensure!(
                effective
                    .get(property)
                    .is_some_and(|value| &value["value"] == expected),
                "intent {name} {property} differs from catalog"
            );
        }
        let wraps = effective.get("wraps").context("intent wraps missing")?["value"]["tuple"]
            .as_array()
            .context("intent wraps must be tuple")?
            .iter()
            .map(|pair| pair["tuple"].clone())
            .collect::<Vec<_>>();
        ensure!(
            Value::Array(wraps) == row["wraps"],
            "intent {name} wraps differ from catalog"
        );
        let declared = effective_fields
            .iter()
            .map(|row| row["name"].as_str().context("field name missing"))
            .collect::<Result<BTreeSet<_>>>()?;
        let properties = row["input_schema"]["properties"]
            .as_object()
            .context("intent properties missing")?;
        ensure!(
            declared == properties.keys().map(String::as_str).collect(),
            "intent {name} dataclass field set differs from catalog"
        );
        let required = effective_fields
            .iter()
            .filter(|row| row["required"].as_bool() == Some(true))
            .map(|row| row["name"].as_str().context("field name missing"))
            .collect::<Result<BTreeSet<_>>>()?;
        ensure!(
            required
                == row["input_schema"]["required"]
                    .as_array()
                    .context("intent required fields missing")?
                    .iter()
                    .map(|name| name.as_str().context("required name invalid"))
                    .collect::<Result<BTreeSet<_>>>()?,
            "intent {name} requiredness differs from catalog"
        );
        fields.push(json!({"name":name,"python_class":class_name,"fields":effective_fields,"provenance":span(&resolver,&class.path,&class.source,class.node.range)}));
        variables.push(json!({"name":name,"python_class":class_name,"values":effective}));
    }
    let registered = classes
        .values()
        .filter(|class| {
            class
                .node
                .decorator_list
                .iter()
                .any(|expr| call_path(expr).as_deref() == Some("register"))
        })
        .map(|class| class.node.name.as_str())
        .collect::<BTreeSet<_>>();
    ensure!(
        registered
            == rows
                .iter()
                .map(|row| row["python_class"].as_str().context("class missing"))
                .collect::<Result<BTreeSet<_>>>()?,
        "source/catalog registered intent class sets differ"
    );
    let document = |rows: Vec<Value>| json!({"format":1,"parity":{"tag":pins["parity"]["tag"].as_str(),"commit":pins["parity"]["commit"].as_str()},"catalog":catalog_provenance,"intents":rows});
    Ok((document(fields), document(variables)))
}

fn compact(value: &Value) -> Result<String> {
    let mut value = value.clone();
    value.sort_all_objects();
    Ok(serde_json::to_string(&value)?)
}

fn str_at(value: &Value, name: &str) -> Result<String> {
    Ok(rust_string(
        value[name]
            .as_str()
            .with_context(|| format!("missing string {name}"))?,
    ))
}
fn opt(value: &Value) -> Result<String> {
    if value.is_null() {
        Ok("None".into())
    } else {
        Ok(format!(
            "Some({})",
            rust_string(value.as_str().context("optional string invalid")?)
        ))
    }
}
fn string_slice(values: &Value) -> Result<String> {
    Ok(format!(
        "&[{}]",
        values
            .as_array()
            .context("string array missing")?
            .iter()
            .map(|value| value
                .as_str()
                .map(rust_string)
                .context("string array item invalid"))
            .collect::<Result<Vec<_>>>()?
            .join(", ")
    ))
}

fn render_specs(
    pins: &DocumentMut,
    intents: &Value,
    reads: &Value,
    fields: &Value,
    vars: &Value,
    read_params: &Value,
) -> Result<Vec<u8>> {
    let provenance = json!({"provenance":{"path":"website/apps/bittensor-website/public/catalog/{intents,reads}.json","commit":pins["parity"]["commit"].as_str()}});
    let mut out = header(&provenance)?;
    out.push_str("//! Complete authenticated intent/read specs with Python defaults and class variables.\n\n");
    out.push_str("/// A Python parameter or dataclass field with an exact serialized extractor value.\n#[derive(Clone, Copy, Debug, Eq, PartialEq)]\npub struct ParamSpec {\n    /// Python parameter/field name.\n    pub name: &'static str,\n    /// Exact source annotation, when declared.\n    pub annotation: Option<&'static str>,\n    /// Catalog JSON type, when this is a read parameter.\n    pub json_type: Option<&'static str>,\n    /// Whether omitting the value fails the Python constructor or CLI read.\n    pub required: bool,\n    /// Whether a read address falls back to the caller's own key.\n    pub own_key: bool,\n    /// Exact JSON representation; None means no Python default.\n    pub default_json: Option<&'static str>,\n    /// Whether the default is produced by a fresh factory.\n    pub default_factory: bool,\n    /// Authenticated declaration source and span as canonical JSON.\n    pub provenance_json: &'static str,\n}\n\n/// An authenticated Python intent and its complete upstream catalog record.\n#[derive(Clone, Copy, Debug, Eq, PartialEq)]\npub struct IntentSpec {\n    /// Stable operation name.\n    pub name: &'static str,\n    /// Catalog summary.\n    pub summary: &'static str,\n    /// Full catalog description.\n    pub description: &'static str,\n    /// Python class name.\n    pub python_class: &'static str,\n    /// Signing key role.\n    pub signer: &'static str,\n    /// Required origin privilege.\n    pub origin: &'static str,\n    /// Read that verifies a privileged effect.\n    pub verify: Option<&'static str>,\n    /// Wrapped pallet/call pairs.\n    pub wraps: &'static [(&'static str, &'static str)],\n    /// Constructor fields in Python declaration order.\n    pub fields: &'static [ParamSpec],\n    /// Fields that accept the all-amount sentinel.\n    pub all_amount_fields: &'static [&'static str],\n    /// Python MEV-shield default.\n    pub mev_shield_default: bool,\n    /// Python mandatory MEV-shield flag.\n    pub mev_shield_required: bool,\n    /// Complete unchanged catalog record as canonical JSON.\n    pub catalog_json: &'static str,\n    /// Effective inherited class variables and their provenance.\n    pub class_vars_json: &'static str,\n}\n\n/// An authenticated read and its complete upstream catalog record.\n#[derive(Clone, Copy, Debug, Eq, PartialEq)]\npub struct ReadSpec {\n    /// Stable read name.\n    pub name: &'static str,\n    /// Catalog summary.\n    pub summary: &'static str,\n    /// Full catalog description.\n    pub description: &'static str,\n    /// Catalog category.\n    pub category: &'static str,\n    /// Namespace accessor from the Python public expression.\n    pub namespace: &'static str,\n    /// Read parameters in Python signature order.\n    pub params: &'static [ParamSpec],\n    /// Complete unchanged catalog record as canonical JSON.\n    pub catalog_json: &'static str,\n    /// Authenticated registration/fetch provenance.\n    pub provenance_json: &'static str,\n}\n\n");
    let params = |items: &[Value], read: bool| -> Result<String> {
        Ok(format!("&[{}]", items.iter().map(|item| {
            let default = &item["default"];
            let present = default["present"].as_bool().context("default presence missing")?;
            Ok(format!("ParamSpec {{ name:{}, annotation:{}, json_type:{}, required:{}, own_key:{}, default_json:{}, default_factory:{}, provenance_json:{} }}",str_at(item,"name")?,opt(&item["annotation"])?,if read {opt(&item["json_type"])?} else {"None".into()},item["required"].as_bool().context("requiredness missing")?,item["own_key"].as_bool().unwrap_or(false),if present {format!("Some({})",rust_string(&compact(&default["value"])?))} else {"None".into()},default["factory"].as_bool().unwrap_or(false),rust_string(&compact(&item["provenance"])?)))
        }).collect::<Result<Vec<_>>>()?.join(", ")))
    };
    out.push_str("/// The complete pinned catalog of 83 intents.\npub static INTENTS: [IntentSpec; 83] = [\n");
    for row in intents.as_array().context("intent rows missing")? {
        let name = row["name"].as_str().context("intent name missing")?;
        let field = fields["intents"]
            .as_array()
            .context("intent fields missing")?
            .iter()
            .find(|field| field["name"].as_str() == Some(name))
            .context("intent fields absent")?;
        let vars = &vars["intents"]
            .as_array()
            .context("intent vars missing")?
            .iter()
            .find(|vars| vars["name"].as_str() == Some(name))
            .context("intent vars absent")?["values"];
        let wraps = row["wraps"]
            .as_array()
            .context("wraps missing")?
            .iter()
            .map(|pair| {
                Ok(format!(
                    "({}, {})",
                    rust_string(pair[0].as_str().context("pallet missing")?),
                    rust_string(pair[1].as_str().context("call missing")?)
                ))
            })
            .collect::<Result<Vec<_>>>()?
            .join(", ");
        out.push_str(&format!("    IntentSpec {{ name:{}, summary:{}, description:{}, python_class:{}, signer:{}, origin:{}, verify:{}, wraps:&[{wraps}], fields:{}, all_amount_fields:{}, mev_shield_default:{}, mev_shield_required:{}, catalog_json:{}, class_vars_json:{} }},\n",str_at(row,"name")?,str_at(row,"summary")?,str_at(row,"description")?,str_at(row,"python_class")?,str_at(row,"signer")?,str_at(row,"origin")?,opt(&row["verify"])?,params(field["fields"].as_array().context("field list missing")?,false)?,string_slice(&vars["all_amount_fields"]["value"]["tuple"] )?,vars["mev_shield_default"]["value"].as_bool().context("shield default missing")?,vars["mev_shield_required"]["value"].as_bool().context("shield required missing")?,rust_string(&compact(row)?),rust_string(&compact(vars)?)));
    }
    out.push_str("];\n\n/// The complete pinned catalog of 102 reads.\npub static READS: [ReadSpec; 102] = [\n");
    let read_params = read_params["reads"]
        .as_array()
        .context("read parameter records missing")?;
    ensure!(
        reads.as_array().context("read rows missing")?.len() == 102,
        "read catalog count changed"
    );
    // The upstream metagraph row deliberately omits its public expression.
    // Derive its namespace from the other authenticated rows in its category.
    let mut namespaces = BTreeMap::new();
    for row in reads.as_array().context("read rows missing")? {
        if let Some(expression) = row["python"].as_str() {
            let namespace = expression
                .strip_prefix("sub.")
                .and_then(|expression| expression.split_once('.'))
                .map(|(namespace, _)| namespace)
                .context("read namespace expression changed")?;
            ensure!(
                expression
                    == format!(
                        "sub.{namespace}.{}(...)",
                        row["name"].as_str().context("read name missing")?
                    ),
                "read public expression names another operation"
            );
            let category = row["category"].as_str().context("read category missing")?;
            ensure!(
                namespaces
                    .insert(category, namespace)
                    .is_none_or(|previous| previous == namespace),
                "catalog category has inconsistent namespaces"
            );
        }
    }
    for row in reads.as_array().context("read rows missing")? {
        let name = row["name"].as_str().context("read name missing")?;
        let parameter = read_params
            .iter()
            .find(|row| row["name"].as_str() == Some(name))
            .context("read defaults absent")?;
        let namespace = namespaces
            .get(row["category"].as_str().context("read category missing")?)
            .context("read category namespace absent")?;
        out.push_str(&format!("    ReadSpec {{ name:{}, summary:{}, description:{}, category:{}, namespace:{}, params:{}, catalog_json:{}, provenance_json:{} }},\n",str_at(row,"name")?,str_at(row,"summary")?,str_at(row,"description")?,str_at(row,"category")?,rust_string(namespace),params(parameter["params"].as_array().context("read params missing")?,true)?,rust_string(&compact(row)?),rust_string(&compact(&parameter["provenance"])?)));
    }
    out.push_str("];\n\n/// Find a stable intent name.\npub fn intent(name: &str) -> Option<&'static IntentSpec> { INTENTS.iter().find(|spec| spec.name == name) }\n\n/// Find a stable read name.\npub fn read(name: &str) -> Option<&'static ReadSpec> { READS.iter().find(|spec| spec.name == name) }\n");
    Ok(out.into_bytes())
}

pub(super) fn outputs(
    root: &Path,
    pins: &DocumentMut,
    tables: &Value,
    read_params: &Value,
) -> Result<Vec<(&'static str, Vec<u8>)>> {
    let (intents, intent_provenance) = artifact(root, pins, INTENTS)?;
    let (reads, _) = artifact(root, pins, READS)?;
    let (fields, variables) = intent_documents(root, pins, &intents, &intent_provenance)?;
    let specs = render_specs(pins, &intents, &reads, &fields, &variables, read_params)?;
    let _ = tables;
    Ok(vec![
        (
            "vendor/upstream/extracted/intent_fields.json",
            pretty(&fields)?,
        ),
        (
            "vendor/upstream/extracted/intent_classvars.json",
            pretty(&variables)?,
        ),
        ("crates/rtao-catalog/src/specs.rs", rustfmt(specs)?),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> Result<std::path::PathBuf> {
        Ok(Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .context("workspace missing")?
            .to_path_buf())
    }

    #[test]
    fn intent_defaults_and_inherited_class_vars_cover_every_catalog_property() -> Result<()> {
        let root = root()?;
        let pins = fs::read_to_string(root.join("upstream.toml"))?.parse::<DocumentMut>()?;
        let (catalog, proof) = artifact(&root, &pins, INTENTS)?;
        let (fields, vars) = intent_documents(&root, &pins, &catalog, &proof)?;
        assert_eq!(
            fields["intents"]
                .as_array()
                .context("intents missing")?
                .len(),
            83
        );
        let lookup = |name: &str, field: &str| -> Result<&Value> {
            fields["intents"]
                .as_array()
                .context("intents missing")?
                .iter()
                .find(|row| row["name"].as_str() == Some(name))
                .context("intent absent")?["fields"]
                .as_array()
                .context("fields missing")?
                .iter()
                .find(|row| row["name"].as_str() == Some(field))
                .context("field absent")
        };
        assert_eq!(
            lookup("add_stake", "slippage_protection")?["default"]["value"],
            json!(true)
        );
        assert_eq!(
            lookup("add_stake", "rate_tolerance")?["default"]["value"],
            json!(0.05)
        );
        assert_eq!(
            lookup("burned_register", "hotkey_ss58")?["default"]["value"],
            Value::Null
        );
        assert_eq!(
            lookup("burned_register", "hotkey_ss58")?["default"]["present"],
            json!(true)
        );
        assert_eq!(lookup("add_stake", "hotkey_ss58")?["required"], json!(true));
        assert_eq!(
            lookup("claim_root", "subnets")?["default"]["factory"],
            json!(true)
        );
        assert_eq!(
            lookup("claim_root", "subnets")?["default"]["value"],
            json!([0])
        );
        assert_eq!(
            lookup("set_subnet_emission_enabled", "netuids")?["required"],
            json!(true)
        );
        let burned = vars["intents"]
            .as_array()
            .context("vars missing")?
            .iter()
            .find(|row| row["name"] == "burned_register")
            .context("burned intent missing")?;
        assert_eq!(
            burned["values"]["mev_shield_required"]["value"],
            json!(true)
        );
        let transfer = vars["intents"]
            .as_array()
            .context("vars missing")?
            .iter()
            .find(|row| row["name"] == "transfer")
            .context("transfer missing")?;
        assert_eq!(
            transfer["values"]["mev_shield_required"]["value"],
            json!(false)
        );
        assert!(
            transfer["values"]["mev_shield_required"]["provenance"]["path"]
                .as_str()
                .is_some_and(|path| path.ends_with("intents/base.py"))
        );
        Ok(())
    }

    #[test]
    fn intent_catalog_mismatches_fail_instead_of_generating_partial_specs() -> Result<()> {
        let root = root()?;
        let pins = fs::read_to_string(root.join("upstream.toml"))?.parse::<DocumentMut>()?;
        let (catalog, proof) = artifact(&root, &pins, INTENTS)?;
        let mut changed = catalog.clone();
        changed[0]["origin"] = json!("root");
        assert!(intent_documents(&root, &pins, &changed, &proof).is_err());
        let mut changed = catalog.clone();
        changed[0]["input_schema"]["properties"]["future"] = json!({"type":"boolean"});
        assert!(intent_documents(&root, &pins, &changed, &proof).is_err());
        let mut changed = catalog;
        changed[0]["wraps"] = json!([]);
        assert!(intent_documents(&root, &pins, &changed, &proof).is_err());
        Ok(())
    }
}
