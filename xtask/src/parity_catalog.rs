// Derived from RaoFoundation/subtensor sdk/python/codegen/check.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Independent catalog contracts and exact pinned call-coverage checks.

use anyhow::{Context, Result, bail, ensure};
use quote::ToTokens;
use rtao_core::{ir::MetadataIr, runtime::Runtime};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};
use syn::parse::Parser;
use toml_edit::{DocumentMut, Item};

pub(super) fn json_file(root: &Path, path: &str) -> Result<Value> {
    let bytes = fs::read(root.join(path))?;
    ensure!(
        bytes.len() <= 64 * 1024 * 1024,
        "parity JSON exceeds bound: {path}"
    );
    serde_json::from_slice(&bytes).with_context(|| format!("parse {path}"))
}
fn equal(actual: &Value, expected: &Value, what: &str) -> Result<()> {
    rtao_primitives::json::compare(actual, expected)
        .with_context(|| format!("catalog mismatch: {what}"))
}

fn nonblank_error_prose<'a>(
    rows: impl IntoIterator<Item = (&'a str, &'a str)>,
    family: &str,
) -> Result<()> {
    for (name, prose) in rows {
        // Python str.strip also removes ASCII U+001C through U+001F.
        ensure!(
            prose.chars().any(|character| {
                !character.is_whitespace() && !matches!(character, '\u{1c}'..='\u{1f}')
            }),
            "{family} error {name} has blank prose"
        );
    }
    Ok(())
}

fn error_name_rules<'a>(
    metadata: &BTreeSet<&str>,
    classified: impl IntoIterator<Item = &'a str>,
    classify: impl Fn(&str) -> rtao_catalog::ErrorCode,
) -> Result<()> {
    for name in classified {
        ensure!(
            metadata.contains(name),
            "classified chain error {name} is absent from pinned metadata"
        );
    }
    for name in metadata {
        ensure!(
            classify(name) != rtao_catalog::ErrorCode::Unknown,
            "pinned metadata error {name} has no semantic classification"
        );
    }
    Ok(())
}
fn text<'a>(row: &'a Value, key: &str) -> Result<&'a str> {
    row[key]
        .as_str()
        .with_context(|| format!("catalog string {key} missing"))
}
fn source_file(root: &Path, path: &str) -> Result<syn::File> {
    syn::parse_file(&fs::read_to_string(root.join(path))?)
        .with_context(|| format!("parse generated catalog surface {path}"))
}

fn unique_names<'a>(
    names: impl IntoIterator<Item = &'a str>,
    what: &str,
) -> Result<BTreeSet<&'a str>> {
    let mut unique = BTreeSet::new();
    for name in names {
        ensure!(!name.is_empty(), "{what} contains an empty name");
        ensure!(unique.insert(name), "{what} contains duplicate name {name}");
    }
    Ok(unique)
}

fn check_names<'a, 'b>(
    actual: impl IntoIterator<Item = &'a str>,
    expected: impl IntoIterator<Item = &'b str>,
    what: &str,
) -> Result<()> {
    let actual = unique_names(actual, what)?;
    let expected = unique_names(expected, what)?;
    ensure!(
        actual == expected,
        "{what} name sets differ: missing {:?}, unexplained {:?}",
        expected.difference(&actual).collect::<Vec<_>>(),
        actual.difference(&expected).collect::<Vec<_>>()
    );
    Ok(())
}

fn named_rows<'a>(rows: &'a Value, what: &str) -> Result<BTreeMap<&'a str, &'a Value>> {
    let mut output = BTreeMap::new();
    for row in rows
        .as_array()
        .with_context(|| format!("{what} must be an array"))?
    {
        let name = text(row, "name")?;
        ensure!(!name.is_empty(), "{what} contains an empty name");
        ensure!(
            output.insert(name, row).is_none(),
            "{what} contains duplicate name {name}"
        );
    }
    Ok(output)
}

fn documentation(attrs: &[syn::Attribute]) -> Result<String> {
    let mut lines = Vec::new();
    for attr in attrs.iter().filter(|attr| attr.path().is_ident("doc")) {
        // doc(alias = ...) identifies the counterpart but does not contribute
        // to its ordinary documentation.
        if let syn::Meta::NameValue(value) = &attr.meta {
            let syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(literal),
                ..
            }) = &value.value
            else {
                bail!("catalog documentation must use literal strings")
            };
            lines.push(literal.value());
        }
    }
    Ok(lines.join("\n"))
}

fn field_documentation_matches(actual: &str, source: &str) -> bool {
    // The interface generator renders these two literal catalog placeholders
    // as HTML entities so rustdoc shows the text instead of parsing a tag.
    // Require that exact representation; do not decode arbitrary entities or
    // normalize whitespace, punctuation, other tags or substantive wording.
    let expected = source
        .replace("<intent name>", "&lt;intent name&gt;")
        .replace("<name>", "&lt;name&gt;");
    actual == expected
}

fn serde_values(attrs: &[syn::Attribute]) -> Result<BTreeMap<String, Value>> {
    let mut output = BTreeMap::new();
    for attr in attrs.iter().filter(|attr| attr.path().is_ident("serde")) {
        let syn::Meta::List(list) = &attr.meta else {
            bail!("serde attribute must have arguments")
        };
        for meta in syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated
            .parse2(list.tokens.clone())?
        {
            match meta {
                syn::Meta::NameValue(value) => {
                    let name = value.path.to_token_stream().to_string();
                    let syn::Expr::Lit(syn::ExprLit {
                        lit: syn::Lit::Str(literal),
                        ..
                    }) = value.value
                    else {
                        bail!("serde schema attributes must use literal strings")
                    };
                    ensure!(
                        output.insert(name, json!(literal.value())).is_none(),
                        "duplicate serde attribute"
                    );
                }
                syn::Meta::Path(path) => {
                    ensure!(
                        output
                            .insert(path.to_token_stream().to_string(), json!(true))
                            .is_none(),
                        "duplicate serde flag"
                    );
                }
                _ => bail!("unsupported serde schema attribute"),
            }
        }
    }
    Ok(output)
}

fn default_value(expression: &syn::Expr) -> Result<Value> {
    match expression {
        syn::Expr::Lit(literal) => match &literal.lit {
            syn::Lit::Str(value) => Ok(json!(value.value())),
            syn::Lit::Bool(value) => Ok(json!(value.value)),
            syn::Lit::Int(value) => {
                serde_json::from_str(value.base10_digits()).context("integer default invalid")
            }
            syn::Lit::Float(value) => Ok(json!(value.base10_parse::<f64>()?)),
            _ => bail!("unsupported literal default"),
        },
        syn::Expr::Path(path) if path.path.is_ident("None") => Ok(Value::Null),
        syn::Expr::MethodCall(call) if call.method == "to_owned" && call.args.is_empty() => {
            default_value(&call.receiver)
        }
        syn::Expr::MethodCall(call) if call.method == "expect" && call.args.len() == 1 => {
            default_value(&call.receiver)
        }
        syn::Expr::MethodCall(call) if call.method == "parse" && call.args.is_empty() => {
            let source = default_value(&call.receiver)?;
            let number: Value = serde_json::from_str(
                source
                    .as_str()
                    .context("numeric default parse needs literal string")?,
            )?;
            ensure!(
                number.is_number(),
                "numeric default parse accepts only exact JSON numbers"
            );
            Ok(number)
        }
        syn::Expr::Call(call) => {
            let path = call.func.to_token_stream().to_string().replace(' ', "");
            ensure!(
                call.args.len() == 1,
                "generated default constructor must have one argument"
            );
            let argument = default_value(call.args.first().context("default argument absent")?)?;
            if path == "rtao_primitives::Amount::parse" {
                ensure!(argument == json!("0"), "unknown native amount default");
                return Ok(json!(0));
            }
            ensure!(
                matches!(
                    path.as_str(),
                    "rtao_primitives::Ratio::new"
                        | "rtao_primitives::NetUid"
                        | "rtao_primitives::MechId"
                        | "Some"
                ),
                "unsupported default constructor {path}"
            );
            Ok(argument)
        }
        syn::Expr::Macro(value) if value.mac.path.is_ident("vec") => Ok(Value::Array(
            syn::punctuated::Punctuated::<syn::Expr, syn::Token![,]>::parse_terminated
                .parse2(value.mac.tokens.clone())?
                .iter()
                .map(default_value)
                .collect::<Result<Vec<_>>>()?,
        )),
        _ => bail!("generated default changed outside checked expression grammar"),
    }
}

fn schema_kinds(schema: &Value) -> Result<BTreeSet<&str>> {
    let kinds = if let Some(kind) = schema.get("type") {
        ensure!(
            schema.get("anyOf").is_none(),
            "ambiguous catalog kind grammar"
        );
        if let Some(kind) = kind.as_str() {
            BTreeSet::from([kind])
        } else {
            unique_names(
                kind.as_array()
                    .context("catalog type must be a string or array")?
                    .iter()
                    .map(|value| value.as_str().context("catalog type must be a string"))
                    .collect::<Result<Vec<_>>>()?,
                "catalog type alternatives",
            )?
        }
    } else {
        schema["anyOf"]
            .as_array()
            .context("catalog kind or anyOf alternatives absent")?
            .iter()
            .map(schema_kinds)
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .flatten()
            .collect()
    };
    ensure!(
        !kinds.is_empty()
            && kinds.iter().all(|kind| matches!(
                *kind,
                "integer" | "number" | "string" | "boolean" | "array" | "object" | "null"
            )),
        "unknown or empty catalog JSON kind"
    );
    Ok(kinds)
}

// Check the catalog's normalized kind, rather than copying the metadata width
// resolver from regen. Regeneration checks exact integer widths. Flexible
// Python carriers retain their documented catalog kind here: WeightsArg also
// accepts mappings, and Value backs the Python dict and numeric/text unions.
fn check_field_kind(ty: &str, annotation: &str, schema: &Value) -> Result<()> {
    let optional_annotation = annotation
        .strip_prefix("Optional[")
        .and_then(|annotation| annotation.strip_suffix(']'));
    let optional_type = ty
        .strip_prefix("Option<")
        .and_then(|ty| ty.strip_suffix('>'));
    ensure!(
        optional_annotation.is_some() == optional_type.is_some(),
        "Rust Option contract differs from Python annotation"
    );
    let ty = optional_type.unwrap_or(ty);
    let annotation = optional_annotation.unwrap_or(annotation);
    let kind = if let Some(item) = ty.strip_prefix("Vec<").and_then(|ty| ty.strip_suffix('>')) {
        if let Some(items) = schema.get("items") {
            let item_annotation = annotation
                .strip_prefix("list[")
                .and_then(|annotation| annotation.strip_suffix(']'))
                .context("catalog array items lack a Python item annotation")?;
            check_field_kind(item, item_annotation, items)?;
        }
        BTreeSet::from(["array"])
    } else {
        match ty {
            "bool" => BTreeSet::from(["boolean"]),
            "String" | "rtao_primitives::AccountId" | "rtao_primitives::Ss58Address" => {
                BTreeSet::from(["string"])
            }
            "u8"
            | "u16"
            | "u32"
            | "u64"
            | "u128"
            | "rtao_primitives::NetUid"
            | "rtao_primitives::MechId"
            | "rtao_primitives::Uid"
            | "rtao_primitives::Percent" => BTreeSet::from(["integer"]),
            "rtao_primitives::Ratio" | "serde_json::Number" => BTreeSet::from(["number"]),
            "rtao_primitives::Amount" => {
                ensure!(
                    annotation == "Money",
                    "amount type lacks Python Money annotation"
                );
                BTreeSet::from(["number", "string"])
            }
            "serde_json::Value" => match annotation {
                "dict" => BTreeSet::from(["object"]),
                "int | float | str" => BTreeSet::from(["integer", "number", "string"]),
                _ => bail!("unchecked flexible JSON carrier annotation {annotation}"),
            },
            "crate::WeightsArg" => {
                ensure!(
                    annotation == "list[float]",
                    "weights carrier annotation differs"
                );
                check_field_kind("rtao_primitives::Ratio", "float", &schema["items"])?;
                BTreeSet::from(["array"])
            }
            _ => bail!("intent Rust type changed outside checked JSON-kind grammar: {ty}"),
        }
    };
    ensure!(
        kind == schema_kinds(schema)?,
        "normalized intent JSON kind differs from catalog"
    );
    Ok(())
}

fn check_interfaces(file: &syn::File, defaults: &syn::File) -> Result<()> {
    let structs = file
        .items
        .iter()
        .filter_map(|item| match item {
            syn::Item::Struct(item) => Some((item.ident.to_string(), item)),
            _ => None,
        })
        .collect::<Vec<_>>();
    check_names(
        structs.iter().map(|(name, _)| name.as_str()),
        rtao_catalog::INTENTS.iter().map(|spec| spec.python_class),
        "intent interface classes",
    )?;
    let structs = structs.into_iter().collect::<BTreeMap<_, _>>();
    let mut default_functions = BTreeMap::new();
    for item in &defaults.items {
        if let syn::Item::Mod(module) = item {
            let body = &module
                .content
                .as_ref()
                .context("defaults modules must be inline")?
                .1;
            for item in body {
                if let syn::Item::Fn(function) = item {
                    let [syn::Stmt::Expr(value, None)] = function.block.stmts.as_slice() else {
                        bail!("default function must have a single value expression")
                    };
                    ensure!(
                        default_functions
                            .insert(
                                (module.ident.to_string(), function.sig.ident.to_string()),
                                default_value(value)?
                            )
                            .is_none(),
                        "duplicate generated default function"
                    );
                }
            }
        }
    }
    let mut expected_defaults = BTreeSet::new();
    for spec in &rtao_catalog::INTENTS {
        let structure = structs
            .get(spec.python_class)
            .with_context(|| format!("missing intent interface {}", spec.python_class))?;
        ensure!(
            serde_values(&structure.attrs)?
                == BTreeMap::from([("deny_unknown_fields".into(), json!(true))]),
            "intent {} must reject unknown fields",
            spec.name
        );
        ensure!(
            documentation(&structure.attrs)? == spec.description,
            "intent {} description differs from Python",
            spec.name
        );
        let schema: Value = serde_json::from_str(spec.catalog_json)?;
        let properties = schema["input_schema"]["properties"]
            .as_object()
            .context("intent input schema properties absent")?;
        check_names(
            properties.keys().map(String::as_str),
            spec.fields.iter().map(|field| field.name),
            &format!("intent {} input properties", spec.name),
        )?;
        check_names(
            schema["input_schema"]["required"]
                .as_array()
                .context("intent required field list absent")?
                .iter()
                .map(|value| value.as_str().context("required field must be string"))
                .collect::<Result<Vec<_>>>()?,
            spec.fields
                .iter()
                .filter(|field| field.required)
                .map(|field| field.name),
            &format!("intent {} required fields", spec.name),
        )?;
        let fields = structure
            .fields
            .iter()
            .map(|field| {
                let identifier = field
                    .ident
                    .as_ref()
                    .context("intent field must be named")?
                    .to_string();
                let serde = serde_values(&field.attrs)?;
                let name = serde
                    .get("rename")
                    .and_then(Value::as_str)
                    .unwrap_or(&identifier)
                    .to_owned();
                Ok((name, (serde, field)))
            })
            .collect::<Result<Vec<_>>>()?;
        check_names(
            fields.iter().map(|(name, _)| name.as_str()),
            spec.fields.iter().map(|field| field.name),
            &format!("intent {} serde fields", spec.name),
        )?;
        let fields = fields.into_iter().collect::<BTreeMap<_, _>>();
        for field in spec.fields {
            let (serde, generated) = &fields[field.name];
            check_field_kind(
                &generated.ty.to_token_stream().to_string().replace(' ', ""),
                field.annotation.context("intent field annotation absent")?,
                &properties[field.name],
            )
            .with_context(|| format!("intent {}.{} input kind", spec.name, field.name))?;
            ensure!(
                serde.keys().all(|key| key == "rename" || key == "default"),
                "intent {}.{} changed its checked serde grammar",
                spec.name,
                field.name
            );
            ensure!(
                field_documentation_matches(
                    &documentation(&generated.attrs)?,
                    properties[field.name]["description"]
                        .as_str()
                        .unwrap_or(field.name),
                ),
                "intent {}.{} field documentation differs from Python",
                spec.name,
                field.name
            );
            if let Some(default) = field.default_json {
                let expected = format!("super::defaults::{}::{}", spec.name, field.name);
                ensure!(
                    serde.get("default").and_then(Value::as_str) == Some(expected.as_str()),
                    "intent {}.{} default wiring differs from extracted Python",
                    spec.name,
                    field.name
                );
                let key = (spec.name.to_owned(), field.name.to_owned());
                equal(
                    default_functions
                        .get(&key)
                        .context("generated default function absent")?,
                    &serde_json::from_str(default)?,
                    &format!("{}.{} default", spec.name, field.name),
                )?;
                expected_defaults.insert(key);
            } else {
                ensure!(
                    !serde.contains_key("default"),
                    "required field {}.{} acquired a default",
                    spec.name,
                    field.name
                );
            }
        }
        let implementations = file
            .items
            .iter()
            .filter_map(|item| match item {
                syn::Item::Impl(item)
                    if item.self_ty.to_token_stream().to_string() == spec.python_class
                        && item.trait_.as_ref().is_some_and(|(_, path, _)| {
                            path.segments
                                .last()
                                .is_some_and(|part| part.ident == "IntentInterface")
                        }) =>
                {
                    Some(item)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        ensure!(
            implementations.len() == 1,
            "intent {} has missing/duplicate interface impl",
            spec.name
        );
        let specification = implementations[0]
            .items
            .iter()
            .find_map(|item| match item {
                syn::ImplItem::Const(item) if item.ident == "SPEC" => Some(&item.expr),
                _ => None,
            })
            .context("intent SPEC pointer absent")?;
        let index = rtao_catalog::INTENTS
            .iter()
            .position(|candidate| candidate.name == spec.name)
            .context("intent catalog index absent")?;
        ensure!(
            specification.to_token_stream().to_string().replace(' ', "")
                == format!("&rtao_catalog::INTENTS[{index}]"),
            "intent {} points at another catalog specification",
            spec.name
        );
        let wraps = implementations[0]
            .items
            .iter()
            .find_map(|item| match item {
                syn::ImplItem::Const(item) if item.ident == "WRAPS" => Some(&item.expr),
                _ => None,
            })
            .context("intent WRAPS descriptor list absent")?;
        let syn::Expr::Reference(reference) = wraps else {
            bail!("intent WRAPS must be a static array reference")
        };
        let syn::Expr::Array(array) = &*reference.expr else {
            bail!("intent WRAPS must be a literal descriptor array")
        };
        let actual = array
            .elems
            .iter()
            .map(|value| value.to_token_stream().to_string().replace(' ', ""))
            .collect::<BTreeSet<_>>();
        let expected = spec
            .wraps
            .iter()
            .map(|(pallet, call)| {
                format!(
                    "&rtao_runtime::calls::{}::{}",
                    snake_name(pallet),
                    call.to_ascii_uppercase()
                )
            })
            .collect::<BTreeSet<_>>();
        ensure!(
            actual == expected && actual.len() == array.elems.len(),
            "intent {} wrapped descriptor set differs from catalog",
            spec.name
        );
        let types = implementations[0]
            .items
            .iter()
            .filter_map(|item| match item {
                syn::ImplItem::Type(item) => Some((
                    item.ident.to_string(),
                    item.ty.to_token_stream().to_string().replace(' ', ""),
                )),
                _ => None,
            })
            .collect::<BTreeMap<_, _>>();
        let role = if spec.signer == "hotkey" {
            "rtao_signer::Hotkey"
        } else {
            "rtao_signer::Coldkey"
        };
        ensure!(
            types.get("Role").map(String::as_str) == Some(role),
            "intent {} signer role differs from Python",
            spec.name
        );
        let origin = match spec.origin {
            "signed" => "rtao_tx::Signed",
            "root" => "rtao_tx::Root",
            "subnet_owner" => "rtao_tx::SubnetOwner",
            _ => bail!("unknown intent origin"),
        };
        ensure!(
            types.get("Origin").map(String::as_str) == Some(origin),
            "intent {} origin differs from Python",
            spec.name
        );
    }
    ensure!(
        default_functions.keys().cloned().collect::<BTreeSet<_>>() == expected_defaults,
        "generated defaults have unexplained names"
    );
    Ok(())
}

fn snake_name(name: &str) -> String {
    let letters = name.chars().collect::<Vec<_>>();
    let mut output = String::new();
    for (index, letter) in letters.iter().enumerate() {
        if letter.is_ascii_uppercase()
            && index > 0
            && (letters[index - 1].is_ascii_lowercase()
                || letters.get(index + 1).is_some_and(char::is_ascii_lowercase))
        {
            output.push('_');
        }
        output.push(letter.to_ascii_lowercase());
    }
    output
}

pub(super) fn catalogs(root: &Path) -> Result<Value> {
    let intents = json_file(root, "vendor/upstream/catalog/intents.json")?;
    let reads = json_file(root, "vendor/upstream/catalog/reads.json")?;
    let errors = json_file(root, "vendor/upstream/catalog/errors.json")?;
    let intents = named_rows(&intents, "raw intent catalog")?;
    check_names(
        intents.keys().copied(),
        rtao_catalog::INTENTS.iter().map(|spec| spec.name),
        "raw and compiled intent registries",
    )?;
    for (name, row) in &intents {
        let spec =
            rtao_catalog::specs::intent(name).context("catalog intent absent from registry")?;
        equal(
            &serde_json::from_str(spec.catalog_json)?,
            row,
            &format!("intent {} full record", spec.name),
        )?;
        ensure!(
            row["summary"].as_str() == Some(spec.summary)
                && row["description"].as_str() == Some(spec.description)
                && row["python_class"].as_str() == Some(spec.python_class)
                && row["signer"].as_str() == Some(spec.signer)
                && row["origin"].as_str() == Some(spec.origin)
                && row["verify"].as_str() == spec.verify,
            "compiled intent {} normalized metadata differs from catalog",
            spec.name
        );
        equal(&json!(spec.wraps), &row["wraps"], "compiled intent wraps")?;
    }
    let reads = named_rows(&reads, "raw read catalog")?;
    check_names(
        reads.keys().copied(),
        rtao_catalog::READS.iter().map(|spec| spec.name),
        "raw and compiled read registries",
    )?;
    let params = json_file(root, "vendor/upstream/extracted/read_params.json")?;
    let read_hits = json_file(root, "vendor/upstream/extracted/read_hits.json")?;
    let params = named_rows(&params["reads"], "extracted read parameter records")?;
    let read_hits = named_rows(&read_hits["reads"], "extracted read-hit records")?;
    for (records, label) in [
        (&params, "extracted read parameters"),
        (&read_hits, "extracted read hits"),
    ] {
        check_names(records.keys().copied(), reads.keys().copied(), label)?;
    }
    let extra =
        fs::read_to_string(root.join("parity/read_hits_extra.toml"))?.parse::<DocumentMut>()?;
    if let Some(reviewed) = extra.get("read") {
        for (name, _) in reviewed
            .as_table()
            .context("reviewed read extras must be a table")?
        {
            ensure!(
                reads.contains_key(name),
                "reviewed extras name unknown read {name}"
            );
        }
    }
    check_names(
        rtao_reads::READ_SPECS.iter().map(|spec| spec.name),
        reads.keys().copied(),
        "generated read registry",
    )?;
    let mut expected_replacements = BTreeSet::new();
    for (name, row) in &reads {
        let spec = rtao_catalog::specs::read(name).context("catalog read absent from registry")?;
        equal(
            &serde_json::from_str(spec.catalog_json)?,
            row,
            &format!("read {} full record", spec.name),
        )?;
        ensure!(
            row["summary"].as_str() == Some(spec.summary)
                && row["description"].as_str() == Some(spec.description)
                && row["category"].as_str() == Some(spec.category),
            "compiled read {} normalized metadata differs from catalog",
            spec.name
        );
        let generated = rtao_reads::READ_SPECS
            .iter()
            .find(|spec| spec.name == *name)
            .context("generated ReadSpec absent")?;
        ensure!(
            generated.summary == spec.summary
                && generated.description == spec.description
                && generated.category.as_str() == spec.category
                && generated.namespace == spec.namespace
                && row["docs_url"].as_str() == Some(generated.docs_url),
            "generated read {} documentation/category/namespace mismatch",
            spec.name
        );
        check_names(
            generated.params.iter().map(|parameter| parameter.name),
            spec.params.iter().map(|parameter| parameter.name),
            &format!("generated read {} parameters", spec.name),
        )?;
        let extracted = named_rows(
            &params[spec.name]["params"],
            &format!("extracted read {} parameters", spec.name),
        )?;
        check_names(
            extracted.keys().copied(),
            spec.params.iter().map(|parameter| parameter.name),
            &format!("extracted read {} parameters", spec.name),
        )?;
        for parameter in spec.params {
            let expected = extracted[parameter.name];
            ensure!(
                expected["required"].as_bool() == Some(parameter.required)
                    && expected["own_key"].as_bool() == Some(parameter.own_key)
                    && expected["json_type"].as_str() == parameter.json_type,
                "read {}.{} parameter contract mismatch",
                spec.name,
                parameter.name
            );
            match parameter.default_json {
                Some(value) => {
                    ensure!(
                        expected["default"]["present"] == json!(true),
                        "read default presence mismatch"
                    );
                    equal(
                        &serde_json::from_str(value)?,
                        &expected["default"]["value"],
                        "read default",
                    )?;
                }
                None => ensure!(
                    expected["default"]["present"] == json!(false),
                    "read acquired default"
                ),
            }
            let generated = generated
                .params
                .iter()
                .find(|field| field.name == parameter.name)
                .context("generated read parameter absent")?;
            let kind = match generated.json_type {
                rtao_reads::JsonType::Integer => "integer",
                rtao_reads::JsonType::Number => "number",
                rtao_reads::JsonType::String => "string",
                rtao_reads::JsonType::Boolean => "boolean",
                rtao_reads::JsonType::Array => "array",
            };
            ensure!(
                parameter.json_type == Some(kind)
                    && parameter.required == generated.required
                    && parameter.own_key == generated.own_key
                    && row["param_docs"][parameter.name]
                        .as_str()
                        .unwrap_or(parameter.name)
                        == generated.doc,
                "generated read {}.{} parameter metadata mismatch",
                spec.name,
                parameter.name
            );
            match (parameter.default_json, generated.default) {
                (Some(expected), Some(actual)) => equal(
                    &serde_json::from_str(actual)?,
                    &serde_json::from_str(expected)?,
                    "generated read default",
                )?,
                (None, None) => {}
                _ => bail!("generated read default presence mismatch"),
            }
        }
        let static_hits = read_hits[spec.name];
        let mut hits = hit_set(&static_hits["hits"], "read static hits")?;
        let reviewed = extra.get("read").and_then(|reads| reads.get(spec.name));
        if let Some(rows) = reviewed.and_then(|read| read.get("hits")) {
            let rows = rows
                .as_array()
                .context("reviewed read extra hits must be an array")?;
            for row in rows {
                let row = row
                    .as_inline_table()
                    .context("reviewed read extra hit must be inline table")?;
                let string = |name: &str| {
                    row.get(name)
                        .and_then(toml_edit::Value::as_str)
                        .with_context(|| format!("extra read hit {name} absent"))
                };
                ensure!(
                    string("reason")?.len() >= 12 && string("python")?.contains("sdk/python/"),
                    "extra read hit needs reason and Python citation"
                );
                ensure!(
                    hits.insert((
                        string("kind")?.into(),
                        string("container")?.into(),
                        string("name")?.into()
                    )),
                    "extra read hit duplicates static or another extra entry"
                );
            }
        }
        // Upstream omits this optional display/provenance list for reads whose
        // complete accesses are discovered in helpers. H still comes from the
        // authenticated source scan and reviewed additions.
        let catalog_hits = match row.get("hits") {
            Some(hits) => hit_set(hits, "catalog read hits")?,
            None => BTreeSet::new(),
        };
        let (effective_catalog, witnesses) = crate::read_hit_replacements::apply(
            root,
            spec.name,
            static_hits,
            &catalog_hits,
            &mut hits,
            reviewed,
        )?;
        ensure!(
            effective_catalog.is_subset(&hits),
            "catalog coverage missing after replacements for {}",
            spec.name
        );
        expected_replacements.extend(witnesses.into_iter().map(|witness| {
            (
                spec.name.to_owned(),
                witness.divergence,
                witness.from,
                witness.to,
            )
        }));
        let generated_hits = generated
            .hits
            .iter()
            .map(|hit| {
                (
                    match hit.kind {
                        rtao_reads::HitKind::Storage => "storage",
                        rtao_reads::HitKind::RuntimeApi => "runtime",
                        rtao_reads::HitKind::Constant => "constant",
                    }
                    .into(),
                    hit.container.into(),
                    hit.name.into(),
                )
            })
            .collect::<BTreeSet<(String, String, String)>>();
        ensure!(
            generated_hits == hits && generated_hits.len() == generated.hits.len(),
            "generated ReadSpec {} has incomplete or unexplained hits",
            spec.name
        );
        equal(
            &render_hint_value(generated.render),
            static_hits
                .get("render")
                .context("extracted read render contract absent")?,
            &format!("read {} render hint", spec.name),
        )?;
        ensure!(
            generated.pinning == read_pinning(text(static_hits, "pinning")?, hits.len(), reviewed)?,
            "generated read {} pinning contract mismatch",
            spec.name
        );
    }
    crate::read_hit_replacements::validate_compiled(
        &expected_replacements,
        rtao_reads::READ_HIT_REPLACEMENTS,
    )?;
    check_names(
        errors["codes"]
            .as_object()
            .context("semantic error catalog absent")?
            .keys()
            .map(String::as_str),
        rtao_catalog::SEMANTIC_ERRORS
            .iter()
            .map(|spec| spec.code.as_str()),
        "raw and compiled semantic error registries",
    )?;
    for spec in &rtao_catalog::SEMANTIC_ERRORS {
        let row = &errors["codes"][spec.code.as_str()];
        ensure!(
            row.is_object() && row["remediation"].as_str() == Some(spec.remediation),
            "semantic code/remediation mismatch"
        );
    }
    check_names(
        errors["chain_errors"]
            .as_object()
            .context("chain errors absent")?
            .keys()
            .map(String::as_str),
        rtao_catalog::CHAIN_ERRORS.iter().map(|spec| spec.name),
        "raw and compiled chain error registries",
    )?;
    for spec in &rtao_catalog::CHAIN_ERRORS {
        equal(
            &serde_json::from_str(spec.catalog_json)?,
            &errors["chain_errors"][spec.name],
            "chain error full record",
        )?;
    }
    nonblank_error_prose(
        rtao_catalog::CHAIN_ERRORS
            .iter()
            .map(|spec| (spec.name, spec.description)),
        "chain",
    )?;
    nonblank_error_prose(
        rtao_catalog::py::DISPATCH_ERROR_DOCS.iter().copied(),
        "dispatch",
    )?;
    check_interfaces(
        &source_file(root, "crates/rtao-intents/src/generated/interfaces.rs")?,
        &source_file(root, "crates/rtao-intents/src/generated/defaults.rs")?,
    )?;
    Ok(
        json!({"intents":rtao_catalog::INTENTS.len(),"reads":rtao_catalog::READS.len(),"semantic_errors":rtao_catalog::SEMANTIC_ERRORS.len(),"chain_errors":rtao_catalog::CHAIN_ERRORS.len()}),
    )
}

fn hit(value: &Value) -> Result<(String, String, String)> {
    Ok((
        text(value, "kind")?.into(),
        text(value, "container")?.into(),
        text(value, "name")?.into(),
    ))
}

fn hit_set(value: &Value, what: &str) -> Result<BTreeSet<(String, String, String)>> {
    let mut output = BTreeSet::new();
    for value in value
        .as_array()
        .with_context(|| format!("{what} must be an array"))?
    {
        let descriptor = hit(value)?;
        ensure!(
            matches!(descriptor.0.as_str(), "storage" | "runtime" | "constant"),
            "{what} contains an unknown descriptor kind"
        );
        ensure!(
            output.insert(descriptor.clone()),
            "{what} contains duplicate descriptor {descriptor:?}"
        );
    }
    Ok(output)
}

fn render_hint_value(hint: Option<rtao_reads::RenderHint>) -> Value {
    match hint {
        None => Value::Null,
        Some(rtao_reads::RenderHint::Grouped { key }) => {
            json!({"kind":"Grouped", "labels":[key]})
        }
        Some(rtao_reads::RenderHint::Matrix { row, col, value }) => {
            json!({"kind":"Matrix", "labels":[row, col, value]})
        }
    }
}

fn read_pinning(
    source: &str,
    complete_hit_count: usize,
    reviewed: Option<&Item>,
) -> Result<rtao_reads::Pinning> {
    ensure!(
        matches!(source, "single" | "pinned"),
        "unknown extracted read pinning policy"
    );
    let mut reviewed_pinning = false;
    if let Some(reviewed) = reviewed {
        let table = reviewed
            .as_table()
            .context("reviewed read extra must be a table")?;
        ensure!(
            table.iter().all(|(key, _)| matches!(
                key,
                "hits" | "replacements" | "pinning" | "pinning_reason" | "pinning_python"
            )),
            "reviewed read extra contains an unexplained decision"
        );
        if let Some(policy) = table.get("pinning") {
            ensure!(
                policy.as_str() == Some("pinned"),
                "reviewed read pinning may only strengthen the source policy"
            );
            ensure!(
                table
                    .get("pinning_reason")
                    .and_then(Item::as_str)
                    .is_some_and(|reason| reason.trim().len() >= 12)
                    && table
                        .get("pinning_python")
                        .and_then(Item::as_str)
                        .is_some_and(|citation| citation.contains("sdk/python/")),
                "reviewed read pinning needs a reason and Python citation"
            );
            reviewed_pinning = true;
        } else {
            ensure!(
                !table.contains_key("pinning_reason") && !table.contains_key("pinning_python"),
                "reviewed read pinning evidence lacks its policy decision"
            );
        }
    }
    Ok(
        if source == "pinned" || complete_hit_count > 1 || reviewed_pinning {
            rtao_reads::Pinning::Pinned
        } else {
            rtao_reads::Pinning::Single
        },
    )
}

pub(super) fn runtime(root: &Path) -> Result<Runtime> {
    let pins = fs::read_to_string(root.join("upstream.toml"))?.parse::<DocumentMut>()?;
    let pin = &pins["networks"]["finney"]["metadata"];
    let path = pin["v15"].as_str().context("pinned metadata path absent")?;
    ensure!(super::valid_path(path), "unsafe metadata path");
    let bytes = fs::read(root.join(path))?;
    ensure!(
        pin["v15_sha256"].as_str() == Some(format!("{:x}", Sha256::digest(&bytes)).as_str()),
        "parity metadata differs from pinned hash"
    );
    Runtime::parse(
        &bytes,
        u32::try_from(pin["spec"].as_integer().context("metadata spec missing")?)?,
        1,
        42,
    )
    .map_err(Into::into)
}

fn coverage_rules(
    ir: &MetadataIr,
    covered: &BTreeSet<String>,
    raw: &BTreeSet<(String, String)>,
    extra: &BTreeSet<(String, String)>,
    wrapped: &BTreeSet<(String, String)>,
) -> Result<usize> {
    let mut all = BTreeSet::new();
    for pallet in covered {
        let pallet = ir
            .pallets
            .iter()
            .find(|p| &p.name == pallet)
            .context("covered pallet missing from metadata")?;
        all.extend(
            pallet
                .calls
                .iter()
                .map(|call| (pallet.name.clone(), call.name.clone())),
        );
    }
    ensure!(
        raw.is_subset(&all) && extra.is_subset(&all),
        "raw-only register names vanished/unknown calls"
    );
    ensure!(
        raw.is_disjoint(wrapped) && extra.is_disjoint(wrapped) && raw.is_disjoint(extra),
        "call has both raw-only and wrapped status or duplicate raw decisions"
    );
    let missing = all
        .difference(
            &wrapped
                .union(raw)
                .cloned()
                .chain(extra.iter().cloned())
                .collect(),
        )
        .cloned()
        .collect::<Vec<_>>();
    ensure!(
        missing.is_empty(),
        "covered chain calls have no intent or raw-only decision: {missing:?}"
    );
    Ok(all.len())
}

pub(super) fn coverage(root: &Path) -> Result<Value> {
    let tables = json_file(root, "vendor/upstream/extracted/named_tables.json")?;
    let values = &tables["tables"]["sdk/python/codegen/check.py"]["values"];
    let covered = values["COVERED_PALLETS"]["value"]["tuple"]
        .as_array()
        .context("extracted covered pallets absent")?
        .iter()
        .map(|v| {
            v.as_str()
                .map(str::to_owned)
                .context("covered pallet invalid")
        })
        .collect::<Result<BTreeSet<_>>>()?;
    let mut raw = BTreeSet::new();
    for pair in values["RAW_ONLY"]["value"]["dict"]
        .as_array()
        .context("extracted raw-only calls absent")?
    {
        let pallet = pair[0].as_str().context("raw-only pallet invalid")?;
        for call in pair[1]["set"]
            .as_array()
            .context("raw-only call set invalid")?
        {
            raw.insert((
                pallet.to_owned(),
                call.as_str().context("raw-only call invalid")?.to_owned(),
            ));
        }
    }
    let map =
        fs::read_to_string(root.join("parity/raw_only_extra.toml"))?.parse::<DocumentMut>()?;
    let mut extra = BTreeSet::new();
    if let Some(rows) = map
        .get("call")
        .and_then(toml_edit::Item::as_array_of_tables)
    {
        for row in rows {
            let pallet = row
                .get("pallet")
                .and_then(toml_edit::Item::as_str)
                .context("extra raw pallet absent")?;
            let call = row
                .get("name")
                .and_then(toml_edit::Item::as_str)
                .context("extra raw call absent")?;
            ensure!(
                row.get("reason")
                    .and_then(toml_edit::Item::as_str)
                    .is_some_and(|reason| reason.trim().len() >= 12),
                "raw-only extra needs reviewed reason"
            );
            ensure!(
                extra.insert((pallet.to_owned(), call.to_owned())),
                "duplicate extra raw call"
            );
        }
    }
    let wrapped = rtao_catalog::INTENTS
        .iter()
        .flat_map(|spec| {
            spec.wraps
                .iter()
                .map(|(pallet, call)| (pallet.to_string(), call.to_string()))
        })
        .collect::<BTreeSet<_>>();
    let ir = MetadataIr::from_runtime(&runtime(root)?)?;
    let metadata_errors = ir
        .pallets
        .iter()
        .flat_map(|pallet| pallet.errors.iter().map(|error| error.name.as_str()))
        .collect::<BTreeSet<_>>();
    error_name_rules(
        &metadata_errors,
        rtao_catalog::errors::NAME_TO_CODE
            .iter()
            .map(|(name, _)| *name),
        rtao_catalog::errors::classify_chain_error,
    )?;
    let count = coverage_rules(&ir, &covered, &raw, &extra, &wrapped)?;
    for spec in &rtao_catalog::INTENTS {
        if let Some(verify) = spec.verify {
            ensure!(
                rtao_catalog::specs::read(verify).is_some(),
                "intent {} verify read absent",
                spec.name
            );
        }
        ensure!(
            spec.origin != "root" || spec.verify.is_some(),
            "root intent {} lacks verification read",
            spec.name
        );
    }
    Ok(
        json!({"pallets":covered.len(),"calls":count,"raw_only":raw.len(),"raw_only_extra":extra.len(),"wrapped":wrapped.len()}),
    )
}

fn shield_assertion(
    pending: &DocumentMut,
    shield: bool,
    implemented: bool,
    complete: bool,
    strict: bool,
) -> Result<()> {
    let pending = pending
        .get("assertion")
        .and_then(Item::as_table_like)
        .context("chain assertion decisions missing")?;
    ensure!(
        pending.iter().all(|(name, _)| name == "shield_period_max"),
        "unknown pending chain assertion"
    );
    let row = pending.get("shield_period_max");
    if shield {
        ensure!(
            row.is_none(),
            "ShieldPeriod bound test now exists; remove stale pending assertion"
        );
    } else {
        let row=row.context("ShieldPeriod maximum lacks its required ordinary assertion against the extracted runtime bound")?;
        ensure!(
            !implemented && !complete,
            "ShieldPeriod implementation or M4-03 completion requires its maximum assertion"
        );
        ensure!(
            row.get("milestone").and_then(Item::as_str) == Some("M4")
                && row.get("plan").and_then(Item::as_str) == Some("M4-03")
                && row
                    .get("reason")
                    .and_then(Item::as_str)
                    .is_some_and(|reason| reason.len() >= 24),
            "ShieldPeriod pending assertion must cite its explicit M4-03 implementation milestone"
        );
        ensure!(
            !strict,
            "strict chain parity refuses pending ShieldPeriod maximum assertion"
        );
    }
    Ok(())
}

pub(super) fn chain(root: &Path, strict: bool) -> Result<Value> {
    let path = "pallets/subtensor/src/rpc_info/metagraph.rs";
    let tree = super::read_tree(&fs::read_to_string(root.join("vendor/upstream/tree.txt"))?)?;
    let source = fs::read_to_string(root.join("vendor/upstream/src").join(path))?;
    ensure!(
        tree.get(path) == Some(&crate::extract::git_blob(source.as_bytes())),
        "selective chain source differs from authenticated blob"
    );
    let file = syn::parse_file(&source)?;
    let fields = file
        .items
        .iter()
        .find_map(|item| match item {
            syn::Item::Struct(item) if item.ident == "SelectiveMetagraph" => Some(&item.fields),
            _ => None,
        })
        .context("SelectiveMetagraph fields absent")?
        .iter()
        .map(|field| {
            field
                .ident
                .as_ref()
                .map(|name| name.to_string().replace('_', "").to_ascii_lowercase())
                .context("selective fields must be named")
        })
        .collect::<Result<Vec<_>>>()?;
    let variants = file
        .items
        .iter()
        .find_map(|item| match item {
            syn::Item::Enum(item) if item.ident == "SelectiveMetagraphIndex" => {
                Some(&item.variants)
            }
            _ => None,
        })
        .context("selective index absent")?
        .iter()
        .map(|variant| variant.ident.to_string().to_ascii_lowercase())
        .collect::<Vec<_>>();
    ensure!(
        fields == variants,
        "SelectiveMetagraphIndex order differs from SelectiveMetagraph field order"
    );
    let runtime = runtime(root)?;
    let proxies = runtime
        .types
        .types
        .iter()
        .filter(|ty| {
            ty.ty
                .path
                .segments
                .last()
                .is_some_and(|name| name == "ProxyType")
        })
        .collect::<Vec<_>>();
    ensure!(
        proxies.len() == 1,
        "metadata ProxyType missing or ambiguous"
    );
    let scale_info::TypeDef::Variant(proxies) = &proxies[0].ty.type_def else {
        bail!("metadata ProxyType must be enum")
    };
    let tables = json_file(root, "vendor/upstream/extracted/named_tables.json")?;
    let mirror = tables["tables"]["sdk/python/bittensor/intents/proxy.py"]["values"]["PROXY_TYPES"]
        ["value"]["tuple"]
        .as_array()
        .context("Python proxy types absent")?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .context("proxy name invalid")
        })
        .collect::<Result<Vec<_>>>()?;
    ensure!(
        mirror
            == proxies
                .variants
                .iter()
                .map(|variant| variant.name.clone())
                .collect::<Vec<_>>(),
        "Python PROXY_TYPES differs from pinned metadata ProxyType"
    );
    ensure!(
        rtao_catalog::py::MEV_SHIELD_ERA_PERIOD <= rtao_catalog::rt::MAX_SHIELD_ERA_PERIOD,
        "shield default exceeds source-extracted runtime bound"
    );
    let tests = super::symbols::index(root)?;
    let shield = tests.feature_test_bodies.values().any(|body| {
        super::state::identifier(body, "ShieldPeriod")
            && super::state::identifier(body, "MAX_SHIELD_ERA_PERIOD")
            && body.contains("assert")
    });
    let pending =
        fs::read_to_string(root.join("parity/chain_pending.toml"))?.parse::<DocumentMut>()?;
    let mut files = Vec::new();
    super::rust_files(&root.join("crates/rtao-client/src"), &mut files)?;
    let implemented = files
        .iter()
        .map(|path| syn::parse_file(&fs::read_to_string(path)?).map_err(Into::into))
        .collect::<Result<Vec<_>>>()?
        .iter()
        .any(|file| {
            file.items.iter().any(|item| {
                matches!(item,syn::Item::Enum(item) if item.ident=="ShieldPeriod")
                    || matches!(item,syn::Item::Struct(item) if item.ident=="ShieldPeriod")
            })
        });
    let plan = fs::read_to_string(root.join("PLAN.md"))?;
    let complete = plan.lines().any(|line| line.starts_with("- [x] **M4-03**"));
    shield_assertion(&pending, shield, implemented, complete, strict)?;
    // regen independently parses from_index arms and checks every freeze_struct
    // hash against the reviewed model tripwire. Extraction checks all other
    // custom-code, fee-routing, rate-limit and chain-rule outputs.
    crate::regen::run(vec!["--check".into()])?;
    Ok(
        json!({"selective_fields":fields.len(),"proxy_variants":mirror.len(),"freeze_struct":"checked","source_constants":"checked_by_extraction","consumer_assertions":{"shield_period_max":if shield {"checked"}else{"pending_M4-03"}},"pending":usize::from(!shield)}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn metadata_errors_require_current_names_and_non_unknown_classification() -> Result<()> {
        use rtao_catalog::{ErrorCode, errors::classify_chain_error};
        // A heuristic-only metadata name need not appear in NAME_TO_CODE.
        let metadata = BTreeSet::from(["InsufficientBalance", "FreshRateLimit"]);
        error_name_rules(&metadata, ["InsufficientBalance"], classify_chain_error)?;
        let stale = error_name_rules(
            &metadata,
            ["InsufficientBalance", "RemovedChainError"],
            classify_chain_error,
        )
        .err()
        .context("jointly stale classifier/catalog names bypassed metadata")?;
        assert!(stale.to_string().contains("RemovedChainError"));
        let mut changed = metadata.clone();
        changed.insert("FreshUnclassifiedError");
        let unknown = error_name_rules(&changed, ["InsufficientBalance"], classify_chain_error)
            .err()
            .context("new metadata errors need an actual classification")?;
        assert!(unknown.to_string().contains("FreshUnclassifiedError"));
        let unknown = error_name_rules(&metadata, ["InsufficientBalance"], |name| {
            if name == "InsufficientBalance" {
                ErrorCode::Unknown
            } else {
                classify_chain_error(name)
            }
        })
        .err()
        .context("agreement on Unknown is not a chain-error classification")?;
        assert!(unknown.to_string().contains("InsufficientBalance"));
        Ok(())
    }

    #[test]
    fn chain_and_dispatch_error_prose_use_python_nonblank_semantics() -> Result<()> {
        for family in ["chain", "dispatch"] {
            for blank in [
                "",
                " \t\n\r",
                "\u{85}\u{a0}\u{2003}\u{2028}\u{3000}",
                "\u{1c}\u{1d}\u{1e}\u{1f}",
            ] {
                let failure = nonblank_error_prose([("PublicError", blank)], family)
                    .err()
                    .context("Python strips every character in the blank fixture")?;
                assert!(failure.to_string().contains(family));
                assert!(failure.to_string().contains("PublicError"));
            }
            for text in ["visible", " \u{1c} explanation \u{a0}", "\u{200b}", "\0"] {
                nonblank_error_prose([("PublicError", text)], family)?;
            }
        }
        Ok(())
    }

    #[test]
    fn shield_consumer_assertion_pending_has_explicit_milestone_and_expires() -> Result<()> {
        let pending = "[assertion.shield_period_max]\nmilestone = 'M4'\nplan = 'M4-03'\nreason = 'The typed ShieldPeriod maximum assertion belongs to the M4-03 shield pipeline.'\n".parse::<DocumentMut>()?;
        shield_assertion(&pending, false, false, false, false)?;
        for (tested, implemented, complete, strict) in [
            (true, true, true, false),
            (false, true, false, false),
            (false, false, true, false),
            (false, false, false, true),
        ] {
            assert!(shield_assertion(&pending, tested, implemented, complete, strict).is_err());
        }
        let absent = "schema=1\n[assertion]".parse::<DocumentMut>()?;
        assert!(shield_assertion(&absent, false, false, false, false).is_err());
        shield_assertion(&absent, true, true, true, true)?;
        let changed = pending
            .to_string()
            .replace("M4-03", "M5-03")
            .parse::<DocumentMut>()?;
        assert!(shield_assertion(&changed, false, false, false, false).is_err());
        Ok(())
    }
    #[test]
    fn registry_mutations_reject_duplicate_replacements_and_name_drift() -> Result<()> {
        let rows = json!([{"name":"one"},{"name":"two"}]);
        let index = named_rows(&rows, "test registry")?;
        check_names(index.keys().copied(), ["one", "two"], "test registries")?;
        let duplicate_replacement = json!([{"name":"one"},{"name":"one"}]);
        assert!(named_rows(&duplicate_replacement, "test registry").is_err());
        assert!(check_names(index.keys().copied(), ["one", "one"], "compiled registry").is_err());
        assert!(check_names(index.keys().copied(), ["one", "new"], "compiled registry").is_err());
        assert!(named_rows(&json!([{"name":""}]), "test registry").is_err());
        Ok(())
    }
    #[test]
    fn render_mutations_reject_missing_changed_and_reordered_labels() -> Result<()> {
        let grouped = render_hint_value(Some(rtao_reads::RenderHint::Grouped { key: "hotkey" }));
        let matrix = render_hint_value(Some(rtao_reads::RenderHint::Matrix {
            row: "uid",
            col: "destination",
            value: "weight",
        }));
        equal(
            &grouped,
            &json!({"kind":"Grouped","labels":["hotkey"]}),
            "grouped",
        )?;
        equal(
            &matrix,
            &json!({"kind":"Matrix","labels":["uid","destination","weight"]}),
            "matrix",
        )?;
        assert!(equal(&grouped, &Value::Null, "missing render").is_err());
        assert!(
            equal(
                &grouped,
                &json!({"kind":"Grouped","labels":["coldkey"]}),
                "changed render"
            )
            .is_err()
        );
        assert!(
            equal(
                &matrix,
                &json!({"kind":"Matrix","labels":["destination","uid","weight"]}),
                "reordered render"
            )
            .is_err()
        );
        Ok(())
    }
    #[test]
    fn descriptor_mutations_reject_duplicate_and_unknown_hits() -> Result<()> {
        let descriptor = json!({"kind":"storage","container":"SubtensorModule","name":"Uids"});
        assert_eq!(hit_set(&json!([descriptor.clone()]), "test hits")?.len(), 1);
        assert!(hit_set(&json!([descriptor.clone(), descriptor]), "test hits").is_err());
        assert!(
            hit_set(
                &json!([{"kind":"rpc","container":"SubtensorModule","name":"Uids"}]),
                "test hits"
            )
            .is_err()
        );
        Ok(())
    }
    #[test]
    fn pinning_reviews_strengthen_with_evidence_and_cannot_weaken() -> Result<()> {
        use rtao_reads::Pinning;
        assert_eq!(read_pinning("single", 1, None)?, Pinning::Single);
        assert_eq!(read_pinning("pinned", 1, None)?, Pinning::Pinned);
        assert_eq!(read_pinning("single", 2, None)?, Pinning::Pinned);
        let review = "[read.block_info]\npinning='pinned'\npinning_reason='Client aggregates block and timestamp lookups.'\npinning_python='sdk/python/bittensor/client.py:349-372'\n";
        let valid = review.parse::<DocumentMut>()?;
        assert_eq!(
            read_pinning("single", 1, Some(&valid["read"]["block_info"]))?,
            Pinning::Pinned
        );
        for changed in [
            review.replace("pinning='pinned'", "pinning='single'"),
            review.replace("sdk/python/bittensor/client.py:349-372", "uncited"),
            review.replace("pinning_reason=", "unexplained="),
            review.replace("pinning='pinned'\n", ""),
        ] {
            let changed = changed.parse::<DocumentMut>()?;
            assert!(read_pinning("single", 1, Some(&changed["read"]["block_info"])).is_err());
        }
        assert!(read_pinning("unknown", 1, None).is_err());
        Ok(())
    }
    #[test]
    fn field_docs_require_exact_rustdoc_placeholder_escaping() {
        for (source, rendered) in [
            (
                "JSON object {\"op\": <intent name>, ...args}.",
                "JSON object {\"op\": &lt;intent name&gt;, ...args}.",
            ),
            (
                "JSON object {\"op\": <name>, ...args}.",
                "JSON object {\"op\": &lt;name&gt;, ...args}.",
            ),
            (
                "Use <intent name> or <name>.",
                "Use &lt;intent name&gt; or &lt;name&gt;.",
            ),
            ("Amount & fraction < 1.", "Amount & fraction < 1."),
            ("Already &lt;name&gt;.", "Already &lt;name&gt;."),
        ] {
            assert!(field_documentation_matches(rendered, source));
            for mutation in [
                format!("{rendered} "),
                rendered.replacen(".", "!", 1),
                rendered.replacen("&lt;", "&amp;lt;", 1),
                rendered.replacen("&gt;", ">", 1),
                rendered.replacen("name", "other", 1),
            ] {
                if mutation != rendered {
                    assert!(
                        !field_documentation_matches(&mutation, source),
                        "accepted {mutation:?}"
                    );
                }
            }
            if source != rendered {
                assert!(!field_documentation_matches(source, source));
            }
        }
        // No general HTML entity decoding: these pairs have distinct rendered
        // or literal wording and must never become equivalent through a gate.
        assert!(!field_documentation_matches("a & b", "a &amp; b"));
        assert!(!field_documentation_matches("<other>", "&lt;other&gt;"));
    }
    #[test]
    fn interface_mutations_reject_requiredness_defaults_and_signer_drift() -> Result<()> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .context("workspace absent")?;
        let source =
            fs::read_to_string(root.join("crates/rtao-intents/src/generated/interfaces.rs"))?;
        let defaults = source_file(root, "crates/rtao-intents/src/generated/defaults.rs")?;
        check_interfaces(&syn::parse_file(&source)?, &defaults)?;
        for changed in [
            source.replacen(
                "type Role = rtao_signer::Coldkey;",
                "type Role = rtao_signer::Hotkey;",
                1,
            ),
            source.replacen("pub amount_alpha:", "pub unexpected_amount:", 1),
            source.replacen(
                "super::defaults::add_collateral::rate_tolerance",
                "super::defaults::add_stake::rate_tolerance",
                1,
            ),
            source.replacen(
                "pub amount_alpha: rtao_primitives::Amount",
                "pub amount_alpha: rtao_primitives::Ratio",
                1,
            ),
            source.replacen(
                "pub netuid: rtao_primitives::NetUid",
                "pub netuid: String",
                1,
            ),
            source.replacen(
                "pub hotkey_ss58: Option<rtao_primitives::Ss58Address>",
                "pub hotkey_ss58: rtao_primitives::Ss58Address",
                1,
            ),
            source.replacen(
                "Subnet to lock collateral on.",
                "Different field documentation.",
                1,
            ),
            source.replacen("&lt;intent name&gt;", "<intent name>", 1),
            source.replacen("&lt;intent name&gt;", "&lt;other name&gt;", 1),
            source.replacen("&lt;name&gt;", "&amp;lt;name&amp;gt;", 1),
        ] {
            ensure!(
                changed != source,
                "interface mutation target no longer exists"
            );
            assert!(check_interfaces(&syn::parse_file(&changed)?, &defaults).is_err());
        }
        assert!(default_value(&syn::parse_str("arbitrary_function()")?).is_err());
        Ok(())
    }
    #[test]
    fn catalog_contracts_match_complete_pinned_records() -> Result<()> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .context("workspace absent")?;
        let checked = catalogs(root)?;
        assert_eq!(checked["intents"], json!(83));
        assert_eq!(checked["reads"], json!(102));
        Ok(())
    }
    #[test]
    fn raw_call_decisions_detect_added_calls_stale_rows_and_double_status() -> Result<()> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .context("workspace absent")?;
        let mut ir = MetadataIr::from_runtime(&runtime(root)?)?;
        let pallet = ir
            .pallets
            .iter_mut()
            .find(|p| !p.calls.is_empty())
            .context("call pallet absent")?;
        let pair = (pallet.name.clone(), pallet.calls[0].name.clone());
        let covered = BTreeSet::from([pair.0.clone()]);
        let raw = pallet
            .calls
            .iter()
            .map(|call| (pair.0.clone(), call.name.clone()))
            .collect::<BTreeSet<_>>();
        coverage_rules(&ir, &covered, &raw, &BTreeSet::new(), &BTreeSet::new())?;
        let mut invalid = raw.clone();
        invalid.insert((pair.0.clone(), "removed".into()));
        assert!(
            coverage_rules(&ir, &covered, &invalid, &BTreeSet::new(), &BTreeSet::new()).is_err()
        );
        assert!(
            coverage_rules(
                &ir,
                &covered,
                &raw,
                &BTreeSet::new(),
                &BTreeSet::from([pair])
            )
            .is_err()
        );
        let pallet = ir
            .pallets
            .iter_mut()
            .find(|p| covered.contains(&p.name))
            .context("pallet absent")?;
        let mut call = pallet.calls[0].clone();
        call.name = "upstream_added_call".into();
        pallet.calls.push(call);
        assert!(coverage_rules(&ir, &covered, &raw, &BTreeSet::new(), &BTreeSet::new()).is_err());
        Ok(())
    }
}
