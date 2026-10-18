//! Generated semantic errors and configuration vocabulary.

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

use super::{Binding, Resolver, header, line, pretty, rust_string, string_table, value};

const ERRORS_PY: &str = "sdk/python/bittensor/error_map.py";
const RESULT_PY: &str = "sdk/python/bittensor/result.py";
const CONFIG_PY: &str = "sdk/python/bittensor/config.py";
const CATALOG: &str = "vendor/upstream/catalog/errors.json";

pub(super) fn variant(name: &str) -> Result<String> {
    ensure!(
        !name.is_empty() && name.bytes().all(|b| b.is_ascii_uppercase() || b == b'_'),
        "invalid Python error enum name {name}"
    );
    Ok(name
        .split('_')
        .map(|word| {
            let mut letters = word.chars();
            letters.next().map_or_else(String::new, |first| {
                format!("{}{}", first, letters.as_str().to_ascii_lowercase())
            })
        })
        .collect())
}

fn enum_document(resolver: &Resolver<'_>, pins: &DocumentMut) -> Result<Value> {
    let source = resolver.source(ERRORS_PY)?;
    let suite =
        ast::Suite::parse(&source, ERRORS_PY).map_err(|error| anyhow::anyhow!("{error}"))?;
    let class = suite
        .iter()
        .find_map(|stmt| match stmt {
            ast::Stmt::ClassDef(class) if class.name.as_str() == "ErrorCode" => Some(class),
            _ => None,
        })
        .context("Python ErrorCode class missing")?;
    ensure!(
        class.decorator_list.is_empty()
            && class.bases.len() == 2
            && matches!(&class.bases[0],ast::Expr::Name(name) if name.id.as_str()=="str")
            && matches!(&class.bases[1],ast::Expr::Name(name) if name.id.as_str()=="Enum"),
        "ErrorCode must remain the plain str/Enum class"
    );
    let mut values = serde_json::Map::new();
    let mut tokens = BTreeSet::new();
    for stmt in &class.body {
        let ast::Stmt::Assign(assign) = stmt else {
            bail!(
                "{ERRORS_PY}:{}: ErrorCode has unsupported body",
                line(&source, stmt.range().start().to_usize())
            );
        };
        ensure!(
            assign.targets.len() == 1,
            "ErrorCode must assign one enum member"
        );
        let ast::Expr::Name(name) = &assign.targets[0] else {
            bail!("ErrorCode member target must be a name");
        };
        let ast::Expr::Constant(constant) = &*assign.value else {
            bail!("ErrorCode member must have a literal string");
        };
        let ast::Constant::Str(token) = &constant.value else {
            bail!("ErrorCode member must have a string token");
        };
        ensure!(
            name.id.as_str().to_ascii_lowercase() == *token,
            "ErrorCode enum name and token must agree"
        );
        ensure!(
            tokens.insert(token.clone()),
            "duplicate ErrorCode token {token}"
        );
        values.insert(name.id.to_string(),json!({"value":token,"rust_variant":variant(name.id.as_str())?,"line":line(&source,stmt.range().start().to_usize()),"end_line":line(&source,stmt.range().end().to_usize())}));
    }
    Ok(
        json!({"format":1,"provenance":{"path":ERRORS_PY,"blob":resolver.tree.get(ERRORS_PY),"tag":pins["parity"]["tag"].as_str(),"commit":pins["parity"]["commit"].as_str()},"values":values}),
    )
}

fn catalog(root: &Path, pins: &DocumentMut) -> Result<(Value, Value)> {
    let artifacts = pins["artifacts"]
        .as_table_like()
        .context("artifact pins missing")?;
    let (source, row) = artifacts
        .iter()
        .find(|(_, row)| row.get("to").and_then(|item| item.as_str()) == Some(CATALOG))
        .context("error catalog artifact pin missing")?;
    let expected = row
        .get("sha256")
        .and_then(|item| item.as_str())
        .context("error catalog hash missing")?;
    let bytes = fs::read(root.join(CATALOG))?;
    ensure!(
        format!("{:x}", Sha256::digest(&bytes)) == expected,
        "{CATALOG}: catalog differs from artifact pin"
    );
    Ok((
        serde_json::from_slice(&bytes)?,
        json!({"path":source,"sha256":expected,"tag":pins["parity"]["tag"].as_str(),"commit":pins["parity"]["commit"].as_str()}),
    ))
}

fn dictionary<'a>(tables: &'a Value, path: &str, name: &str) -> Result<&'a Vec<Value>> {
    tables
        .get(path)
        .and_then(|table| table.get("values"))
        .and_then(|values| values.get(name))
        .and_then(|entry| entry.get("value"))
        .and_then(|value| value.get("dict"))
        .and_then(Value::as_array)
        .with_context(|| format!("missing extracted {path}::{name}"))
}

fn named_table_lookup(expr: &ast::Expr, table: &str) -> bool {
    matches!(expr, ast::Expr::Subscript(lookup)
        if matches!(&*lookup.value, ast::Expr::Name(name) if name.id.as_str() == table)
            && matches!(&*lookup.slice, ast::Expr::Name(name) if name.id.as_str() == "name"))
}

fn classifier_bindings(
    resolver: &mut Resolver<'_>,
    enumeration: &Value,
    tables: &Value,
) -> Result<Value> {
    resolver.module(RESULT_PY)?;
    let bindings = &resolver.modules[RESULT_PY].bindings;
    let mut checked = serde_json::Map::new();
    for (alias, name) in [
        ("_NAME_TO_CODE", "NAME_TO_CODE"),
        ("_DISPATCH_ERRORS", "DISPATCH_ERRORS"),
        ("_CUSTOM_TRANSACTION_ERRORS", "CUSTOM_TRANSACTION_ERRORS"),
        ("_INVALID_TRANSACTION_ERRORS", "INVALID_TRANSACTION_ERRORS"),
    ] {
        let Some(Binding::Value(actual)) = bindings.get(alias) else {
            bail!("{RESULT_PY}: classifier binding {alias} is not a resolved table");
        };
        ensure!(
            actual.json()? == tables[ERRORS_PY]["values"][name]["value"],
            "{RESULT_PY}: classifier binding {alias} differs from {ERRORS_PY}::{name}"
        );
        checked.insert(
            alias.into(),
            json!({"matches_path":ERRORS_PY,"matches_name":name}),
        );
    }
    let Some(Binding::Namespace(actual)) = bindings.get("ErrorCode") else {
        bail!("{RESULT_PY}: ErrorCode is not a resolved enum");
    };
    let expected = enumeration["values"]
        .as_object()
        .context("error enum values missing")?;
    ensure!(
        actual.len() == expected.len(),
        "{RESULT_PY}: ErrorCode members differ"
    );
    for (name, row) in expected {
        let Some(Binding::Value(actual)) = actual.get(name) else {
            bail!("{RESULT_PY}: ErrorCode member {name} is not resolved");
        };
        ensure!(
            actual.json()? == row["value"],
            "{RESULT_PY}: ErrorCode member {name} differs"
        );
    }
    checked.insert(
        "ErrorCode".into(),
        json!({"matches_path":ERRORS_PY,"matches_name":"ErrorCode"}),
    );
    Ok(Value::Object(checked))
}

fn classifier_rules(resolver: &Resolver<'_>) -> Result<Value> {
    let source = resolver.source(RESULT_PY)?;
    let suite =
        ast::Suite::parse(&source, RESULT_PY).map_err(|error| anyhow::anyhow!("{error}"))?;
    let function = suite
        .iter()
        .find_map(|stmt| match stmt {
            ast::Stmt::FunctionDef(function) if function.name.as_str() == "classify_error" => {
                Some(function)
            }
            _ => None,
        })
        .context("classify_error missing")?;
    let body=function.body.iter().filter(|stmt|!matches!(stmt,ast::Stmt::Expr(expr) if matches!(&*expr.value,ast::Expr::Constant(value) if matches!(&value.value,ast::Constant::Str(_))))).collect::<Vec<_>>();
    ensure!(
        body.len() == 4,
        "{RESULT_PY}:{}: classify_error body form changed",
        line(&source, function.range.start().to_usize())
    );
    let ast::Stmt::If(named) = body[0] else {
        bail!("classify_error named branch missing");
    };
    ensure!(
        matches!(&*named.test,ast::Expr::Name(name) if name.id.as_str()=="name")
            && named.orelse.is_empty()
            && named.body.len() == 4,
        "classify_error named precedence changed"
    );
    for (stmt, table) in named.body[..2]
        .iter()
        .zip(["_NAME_TO_CODE", "_DISPATCH_ERRORS"])
    {
        let ast::Stmt::If(branch) = stmt else {
            bail!("classify_error table lookup missing");
        };
        ensure!(
            matches!(&*branch.test,ast::Expr::Compare(compare) if matches!(&*compare.left,ast::Expr::Name(name) if name.id.as_str()=="name") && compare.ops==[ast::CmpOp::In] && compare.comparators.len()==1 && matches!(&compare.comparators[0],ast::Expr::Name(name) if name.id.as_str()==table))
                && branch.body.len() == 1
                && branch.orelse.is_empty(),
            "classify_error exact-name lookup changed"
        );
        let ast::Stmt::Return(ret) = &branch.body[0] else {
            bail!("classify_error exact-name lookup must return its code");
        };
        let Some(returned) = &ret.value else {
            bail!("classify_error exact-name lookup has no return value");
        };
        ensure!(
            if table == "_NAME_TO_CODE" {
                named_table_lookup(returned, table)
            } else {
                matches!(&**returned, ast::Expr::Subscript(first)
                    if named_table_lookup(&first.value, table)
                        && matches!(&*first.slice, ast::Expr::Constant(value)
                            if matches!(&value.value, ast::Constant::Int(index) if index.to_string() == "0")))
            },
            "classify_error exact-name return changed"
        );
    }
    let ast::Stmt::If(rate) = &named.body[2] else {
        bail!("rate-limit rule missing");
    };
    let ast::Expr::BoolOp(condition) = &*rate.test else {
        bail!("rate-limit condition must be or");
    };
    ensure!(
        condition.op == ast::BoolOp::Or
            && condition.values.len() == 2
            && rate.body.len() == 1
            && rate.orelse.is_empty(),
        "rate-limit condition changed"
    );
    let ast::Expr::Call(suffix) = &condition.values[0] else {
        bail!("rate-limit suffix missing");
    };
    ensure!(
        super::call_path(&suffix.func).as_deref() == Some("name.endswith")
            && suffix.args.len() == 1
            && suffix.keywords.is_empty(),
        "rate-limit suffix call changed"
    );
    let ast::Expr::Constant(suffix) = &suffix.args[0] else {
        bail!("rate-limit suffix must be literal");
    };
    let ast::Constant::Str(suffix) = &suffix.value else {
        bail!("rate-limit suffix must be string");
    };
    let ast::Expr::Compare(contains) = &condition.values[1] else {
        bail!("rate-limit contains missing");
    };
    ensure!(
        contains.ops == [ast::CmpOp::In]
            && contains.comparators.len() == 1
            && matches!(&contains.comparators[0],ast::Expr::Name(name) if name.id.as_str()=="name"),
        "rate-limit contains condition changed"
    );
    let ast::Expr::Constant(needle) = &*contains.left else {
        bail!("rate-limit needle must be literal");
    };
    let ast::Constant::Str(needle) = &needle.value else {
        bail!("rate-limit needle must be string");
    };
    ensure!(
        matches!(&rate.body[0],ast::Stmt::Return(ret) if ret.value.as_ref().is_some_and(|value|super::call_path(value).as_deref()==Some("ErrorCode.RATE_LIMITED")))
            && matches!(&named.body[3],ast::Stmt::Return(ret) if ret.value.as_ref().is_some_and(|value|super::call_path(value).as_deref()==Some("ErrorCode.UNKNOWN"))),
        "classify_error named returns changed"
    );
    let ast::Stmt::Assign(lower) = body[1] else {
        bail!("classify_error lowercasing missing");
    };
    ensure!(
        lower.targets.len() == 1
            && matches!(&lower.targets[0],ast::Expr::Name(name) if name.id.as_str()=="haystack"),
        "classify_error haystack changed"
    );
    let ast::Expr::Call(lower) = &*lower.value else {
        bail!("classify_error must call lower");
    };
    let ast::Expr::Attribute(method) = &*lower.func else {
        bail!("classify_error must use text.lower");
    };
    ensure!(
        method.attr.as_str() == "lower"
            && lower.args.is_empty()
            && lower.keywords.is_empty()
            && matches!(&*method.value,ast::Expr::BoolOp(or) if or.op==ast::BoolOp::Or && or.values.len()==2 && matches!(&or.values[0],ast::Expr::Name(name) if name.id.as_str()=="text") && matches!(&or.values[1],ast::Expr::Constant(value) if matches!(&value.value,ast::Constant::Str(value) if value.is_empty()))),
        "classify_error text normalization changed"
    );
    let ast::Stmt::For(fallback) = body[2] else {
        bail!("classify_error fallback loop missing");
    };
    ensure!(
        matches!(&*fallback.iter,ast::Expr::Name(name) if name.id.as_str()=="_SUBSTRING_FALLBACK")
            && matches!(&*fallback.target, ast::Expr::Tuple(target)
                if target.elts.len() == 2
                    && matches!(&target.elts[0], ast::Expr::Name(name) if name.id.as_str() == "needle")
                    && matches!(&target.elts[1], ast::Expr::Name(name) if name.id.as_str() == "code"))
            && fallback.body.len() == 1
            && fallback.orelse.is_empty(),
        "classify_error fallback iteration changed"
    );
    let ast::Stmt::If(matching) = &fallback.body[0] else {
        bail!("classify_error fallback match missing");
    };
    ensure!(
        matches!(&*matching.test,ast::Expr::Compare(compare) if compare.ops==[ast::CmpOp::In] && super::call_path(&compare.left).as_deref()==Some("needle") && compare.comparators.len()==1 && super::call_path(&compare.comparators[0]).as_deref()==Some("haystack"))
            && matching.body.len() == 1
            && matching.orelse.is_empty()
            && matches!(&matching.body[0],ast::Stmt::Return(ret) if ret.value.as_ref().is_some_and(|value|super::call_path(value).as_deref()==Some("code")))
            && matches!(body[3],ast::Stmt::Return(ret) if ret.value.as_ref().is_some_and(|value|super::call_path(value).as_deref()==Some("ErrorCode.UNKNOWN"))),
        "classify_error fallback return changed"
    );
    Ok(
        json!({"rate_limit_suffix":suffix,"rate_limit_contains":needle,"line":line(&source,function.range.start().to_usize()),"end_line":line(&source,function.range.end().to_usize())}),
    )
}

fn render_errors(
    enumeration: &Value,
    catalog: &Value,
    tables: &Value,
    rules: &Value,
) -> Result<Vec<u8>> {
    let mut out = header(enumeration)?;
    out.push_str("//! Semantic error codes and the pinned Python classification tables.\n\n");
    let values = enumeration["values"]
        .as_object()
        .context("error enum values missing")?;
    let variants = values
        .values()
        .map(|row| {
            Ok((
                row["value"]
                    .as_str()
                    .context("error token missing")?
                    .to_owned(),
                row["rust_variant"]
                    .as_str()
                    .context("Rust variant missing")?
                    .to_owned(),
            ))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;
    let codes = catalog["codes"]
        .as_object()
        .context("catalog codes missing")?;
    ensure!(
        codes.keys().collect::<BTreeSet<_>>() == variants.keys().collect::<BTreeSet<_>>(),
        "Python ErrorCode and catalog semantic codes differ"
    );
    out.push_str("/// Semantic classification of chain and client errors.\n#[doc(alias = \"bittensor.result.ErrorCode\")]\n#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, serde::Serialize, serde::Deserialize)]\n#[serde(rename_all = \"snake_case\")]\n#[non_exhaustive]\npub enum ErrorCode {\n");
    for (token, name) in &variants {
        out.push_str(&format!("    /// Python `{token}`.\n    {name},\n"));
    }
    out.push_str("}\n\nimpl ErrorCode {\n    /// The Python semantic error token.\n    pub const fn as_str(self) -> &'static str {\n        match self {\n");
    for (token, name) in &variants {
        out.push_str(&format!(
            "            Self::{name} => {},\n",
            rust_string(token)
        ));
    }
    out.push_str("        }\n    }\n\n    /// Parse an exact Python semantic error token.\n    pub fn from_code(code: &str) -> Option<Self> {\n        match code {\n");
    for (token, name) in &variants {
        out.push_str(&format!(
            "            {} => Some(Self::{name}),\n",
            rust_string(token)
        ));
    }
    out.push_str("            _ => None,\n        }\n    }\n}\n\nimpl std::fmt::Display for ErrorCode {\n    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(self.as_str()) }\n}\n\n");
    let names = dictionary(tables, ERRORS_PY, "NAME_TO_CODE")?;
    let chain = catalog["chain_errors"]
        .as_object()
        .context("catalog chain errors missing")?;
    let extracted = names
        .iter()
        .map(|pair| pair[0].as_str().context("chain name missing"))
        .collect::<Result<BTreeSet<_>>>()?;
    ensure!(
        extracted == chain.keys().map(String::as_str).collect::<BTreeSet<_>>(),
        "NAME_TO_CODE differs from catalog chain names"
    );
    out.push_str("/// Exact chain names and semantic classifications.\n#[doc(alias = \"bittensor.error_map.NAME_TO_CODE\")]\npub const NAME_TO_CODE: &[(&str, ErrorCode)] = &[\n");
    for pair in names {
        let name = pair[0].as_str().context("chain name missing")?;
        let code = pair[1].as_str().context("chain code missing")?;
        ensure!(
            chain[name]["code"].as_str() == Some(code),
            "catalog code differs for {name}"
        );
        out.push_str(&format!(
            "    ({}, ErrorCode::{}),\n",
            rust_string(name),
            variants
                .get(code)
                .with_context(|| format!("unknown code {code}"))?
        ));
    }
    out.push_str("];\n\n/// Upstream semantic-code documentation.\n#[derive(Clone, Copy, Debug, Eq, PartialEq)]\npub struct SemanticErrorSpec {\n/// Semantic code.\npub code: ErrorCode,\n/// Exact Python remediation.\npub remediation: &'static str,\n/// Exact Python long-form explanation.\npub explanation: &'static str,\n/// Catalog documentation URL.\npub docs_url: &'static str,\n/// Catalog Markdown URL.\npub markdown_url: &'static str,\n}\n\n/// Exact chain-error documentation and source references.\n#[derive(Clone, Copy, Debug, Eq, PartialEq)]\npub struct ChainErrorSpec {\n/// Exact chain variant name.\npub name: &'static str,\n/// Semantic classification.\npub code: ErrorCode,\n/// Exact catalog description.\npub description: &'static str,\n/// Catalog documentation URL.\npub docs_url: &'static str,\n/// Catalog Markdown URL.\npub markdown_url: &'static str,\n/// Pallets exposing the error.\npub pallets: &'static [&'static str],\n/// Complete catalog source references as canonical JSON.\npub sources_json: Option<&'static str>,\n/// Complete untouched catalog record as canonical JSON.\npub catalog_json: &'static str,\n}\n\n/// All 18 pinned semantic codes and their Python explanations.\npub static SEMANTIC_ERRORS: [SemanticErrorSpec; 18] = [\n");
    let remediation = dictionary(tables, RESULT_PY, "REMEDIATION")?
        .iter()
        .map(|pair| {
            Ok((
                pair[0].as_str().context("remediation code missing")?,
                pair[1].as_str().context("remediation text missing")?,
            ))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;
    let explanations = dictionary(tables, RESULT_PY, "EXPLANATIONS")?
        .iter()
        .map(|pair| {
            Ok((
                pair[0].as_str().context("explanation code missing")?,
                pair[1].as_str().context("explanation text missing")?,
            ))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;
    ensure!(
        remediation.keys().copied().collect::<BTreeSet<_>>()
            == variants.keys().map(String::as_str).collect()
            && explanations.keys().copied().collect::<BTreeSet<_>>()
                == variants.keys().map(String::as_str).collect(),
        "Python semantic documentation has a different code set"
    );
    for (token, variant) in &variants {
        let info = &codes[token];
        ensure!(
            info["remediation"].as_str() == Some(remediation[token.as_str()]),
            "semantic remediation differs from catalog for {token}"
        );
        out.push_str(&format!("SemanticErrorSpec {{ code:ErrorCode::{variant},remediation:{},explanation:{},docs_url:{},markdown_url:{} }},\n",rust_string(remediation[token.as_str()]),rust_string(explanations[token.as_str()]),rust_string(info["docs_url"].as_str().context("semantic docs URL missing")?),rust_string(info["markdown_url"].as_str().context("semantic Markdown URL missing")?)));
    }
    out.push_str(&format!("];\n\n/// All {count} pinned chain errors and their full catalog descriptions.\npub static CHAIN_ERRORS: [ChainErrorSpec; {count}] = [\n", count = chain.len()));
    for (name, row) in chain {
        let code = row["code"]
            .as_str()
            .context("chain semantic code missing")?;
        let pallets = row["pallets"]
            .as_array()
            .context("chain pallets missing")?
            .iter()
            .map(|pallet| {
                pallet
                    .as_str()
                    .map(rust_string)
                    .context("chain pallet invalid")
            })
            .collect::<Result<Vec<_>>>()?
            .join(", ");
        let sources = match row.get("sources") {
            Some(sources) => {
                ensure!(
                    sources.is_array(),
                    "chain source references must be an array"
                );
                let mut sources = sources.clone();
                sources.sort_all_objects();
                format!("Some({})", rust_string(&serde_json::to_string(&sources)?))
            }
            None => "None".to_owned(),
        };
        let mut catalog_row = row.clone();
        catalog_row.sort_all_objects();
        out.push_str(&format!("ChainErrorSpec {{ name:{},code:ErrorCode::{},description:{},docs_url:{},markdown_url:{},pallets:&[{pallets}],sources_json:{sources},catalog_json:{} }},\n",rust_string(name),variants.get(code).context("chain code invalid")?,rust_string(row["description"].as_str().context("chain description missing")?),rust_string(row["docs_url"].as_str().context("chain docs URL missing")?),rust_string(row["markdown_url"].as_str().context("chain Markdown URL missing")?),rust_string(&serde_json::to_string(&catalog_row)?)));
    }
    out.push_str("];\n\n/// Dispatch names and semantic classifications.\n#[doc(alias = \"bittensor.error_map.DISPATCH_ERRORS\")]\npub const DISPATCH_ERRORS: &[(&str, ErrorCode)] = &[\n");
    for pair in dictionary(tables, ERRORS_PY, "DISPATCH_ERRORS")? {
        let name = pair[0].as_str().context("dispatch name missing")?;
        let code = pair[1]["tuple"][0]
            .as_str()
            .context("dispatch code missing")?;
        out.push_str(&format!(
            "    ({}, ErrorCode::{}),\n",
            rust_string(name),
            variants
                .get(code)
                .context("unknown dispatch semantic code")?
        ));
    }
    out.push_str("];\n\n/// Ordered fallback needles used only when no chain name is available.\n#[doc(alias = \"bittensor.result._SUBSTRING_FALLBACK\")]\npub const SUBSTRING_FALLBACK: &[(&str, ErrorCode)] = &[\n");
    let fallback = tables[RESULT_PY]["values"]["_SUBSTRING_FALLBACK"]["value"]["tuple"]
        .as_array()
        .context("substring fallback missing")?;
    for pair in fallback {
        let needle = pair["tuple"][0]
            .as_str()
            .context("fallback needle missing")?;
        let code = pair["tuple"][1].as_str().context("fallback code missing")?;
        out.push_str(&format!(
            "    ({}, ErrorCode::{}),\n",
            rust_string(needle),
            variants.get(code).context("unknown fallback code")?
        ));
    }
    out.push_str("];\n\n/// Look up an exact chain or dispatch error name.\npub fn chain_error_code(name: &str) -> Option<ErrorCode> {\n    NAME_TO_CODE.iter().chain(DISPATCH_ERRORS).find_map(|(known, code)| (*known == name).then_some(*code))\n}\n\n/// Classify a named error using Python's exact-name and rate-limit rules.\npub fn classify_chain_error(name: &str) -> ErrorCode { classify_error(\"\", Some(name)) }\n\n/// Classify an error with Python's named-before-text precedence.\n#[doc(alias = \"bittensor.result.classify_error\")]\npub fn classify_error(text: &str, name: Option<&str>) -> ErrorCode {\n    if let Some(name) = name.filter(|name| !name.is_empty()) {\n        if let Some(code) = chain_error_code(name) { return code; }\n");
    out.push_str(&format!(
        "        if name.ends_with({}) || name.contains({}) {{ return ErrorCode::RateLimited; }}\n",
        rust_string(
            rules["rate_limit_suffix"]
                .as_str()
                .context("rate suffix missing")?
        ),
        rust_string(
            rules["rate_limit_contains"]
                .as_str()
                .context("rate needle missing")?
        ),
    ));
    out.push_str("        return ErrorCode::Unknown;\n    }\n    let text = text.to_lowercase();\n    SUBSTRING_FALLBACK.iter().find_map(|(needle, code)| text.contains(needle).then_some(*code)).unwrap_or(ErrorCode::Unknown)\n}\n\n");
    out.push_str("/// Subtensor/Substrate custom transaction codes, including the explicit catch-all.\n#[doc(alias = \"bittensor.error_map.CUSTOM_TRANSACTION_ERRORS\")]\npub const CUSTOM_TRANSACTION_ERRORS: &[(u8, &str)] = &[\n");
    for pair in dictionary(tables, ERRORS_PY, "CUSTOM_TRANSACTION_ERRORS")? {
        let code = u8::try_from(pair[0].as_u64().context("custom code must be u8")?)?;
        let name = pair[1].as_str().context("custom code name missing")?;
        out.push_str(&format!("    ({code}, {}),\n", rust_string(name)));
    }
    out.push_str("];\n\n/// Ordered display-text needles for standard invalid transaction errors.\n#[doc(alias = \"bittensor.error_map.INVALID_TRANSACTION_ERRORS\")]\npub const INVALID_TRANSACTION_ERRORS: &[(&str, &str)] = &[\n");
    for pair in tables[ERRORS_PY]["values"]["INVALID_TRANSACTION_ERRORS"]["value"]["tuple"]
        .as_array()
        .context("invalid-transaction table missing")?
    {
        out.push_str(&format!(
            "    ({}, {}),\n",
            rust_string(
                pair["tuple"][0]
                    .as_str()
                    .context("invalid-transaction needle missing")?
            ),
            rust_string(
                pair["tuple"][1]
                    .as_str()
                    .context("invalid-transaction name missing")?
            )
        ));
    }
    out.push_str("];\n\n/// Resolve a custom transaction code through the extracted exact table.\npub fn custom_transaction_name(code: u8) -> Option<&'static str> { CUSTOM_TRANSACTION_ERRORS.iter().find_map(|(known, name)| (*known == code).then_some(*name)) }\n\n/// Resolve a standard invalid-transaction display message, case-insensitively.\npub fn invalid_transaction_name(text: &str) -> Option<&'static str> {\n    let text = text.to_lowercase();\n    INVALID_TRANSACTION_ERRORS.iter().find_map(|(needle, name)| text.contains(needle).then_some(*name))\n}\n");
    Ok(out.into_bytes())
}

fn config_environment(resolver: &Resolver<'_>) -> Result<Value> {
    let source = resolver.source(CONFIG_PY)?;
    let suite =
        ast::Suite::parse(&source, CONFIG_PY).map_err(|error| anyhow::anyhow!("{error}"))?;
    let names = [
        ("config_path", "CONFIG_ENV_VAR", "DEFAULT_CONFIG_PATH"),
        ("proxies_path", "PROXIES_ENV_VAR", "DEFAULT_PROXIES_PATH"),
        (
            "addresses_path",
            "ADDRESSES_ENV_VAR",
            "DEFAULT_ADDRESSES_PATH",
        ),
        (
            "multisigs_path",
            "MULTISIGS_ENV_VAR",
            "DEFAULT_MULTISIGS_PATH",
        ),
        (
            "multisig_cache_path",
            "MULTISIG_CACHE_ENV_VAR",
            "DEFAULT_MULTISIG_CACHE_PATH",
        ),
        (
            "subnet_names_path",
            "SUBNET_NAMES_ENV_VAR",
            "DEFAULT_SUBNET_NAMES_PATH",
        ),
        (
            "token_symbols_path",
            "TOKEN_SYMBOLS_ENV_VAR",
            "DEFAULT_TOKEN_SYMBOLS_PATH",
        ),
    ];
    let mut values = serde_json::Map::new();
    for (function, key, default) in names {
        let function = suite
            .iter()
            .find_map(|stmt| match stmt {
                ast::Stmt::FunctionDef(def) if def.name.as_str() == function => Some(def),
                _ => None,
            })
            .with_context(|| format!("config helper {function} missing"))?;
        ensure!(
            function.body.len() == 1,
            "config path helper must remain one return"
        );
        let ast::Stmt::Return(ret) = &function.body[0] else {
            bail!("config path helper must return Path");
        };
        let Some(expr) = &ret.value else {
            bail!("config path return missing");
        };
        let ast::Expr::Call(path) = &**expr else {
            bail!("config path must call Path");
        };
        ensure!(
            super::call_path(&path.func).as_deref() == Some("Path")
                && path.args.len() == 1
                && path.keywords.is_empty(),
            "config path must call Path with one default expression"
        );
        let ast::Expr::BoolOp(or) = &path.args[0] else {
            bail!("config env/default must be an or expression");
        };
        ensure!(
            or.op == ast::BoolOp::Or
                && or.values.len() == 2
                && matches!(&or.values[1],ast::Expr::Name(name) if name.id.as_str()==default),
            "config env/default form changed"
        );
        let ast::Expr::Call(env) = &or.values[0] else {
            bail!("config env getter missing");
        };
        ensure!(
            super::call_path(&env.func).as_deref() == Some("os.getenv")
                && env.args.len() == 1
                && env.keywords.is_empty(),
            "config must use os.getenv with one name"
        );
        let ast::Expr::Constant(name) = &env.args[0] else {
            bail!("config env name must be literal");
        };
        let ast::Constant::Str(name) = &name.value else {
            bail!("config env name must be string");
        };
        values.insert(key.into(),json!({"value":name,"default":default,"empty_is_unset":true,"line":line(&source,ret.range.start().to_usize()),"end_line":line(&source,ret.range.end().to_usize())}));
    }
    Ok(Value::Object(values))
}

fn render_config(config: &Value, environment: &Value) -> Result<Vec<u8>> {
    let mut out = header(config)?;
    out.push_str("//! Configurable value types, paths, and cache defaults extracted from Python.\n\n/// Builtin coercion type tags accepted by the Python extractor.\n#[derive(Clone, Copy, Debug, Eq, PartialEq)]\npub enum BuiltinType {\n");
    let entries = string_table(config, "SETTABLE")?;
    let tags = entries
        .iter()
        .map(|(_, kind)| kind["type"].as_str().context("config type tag missing"))
        .collect::<Result<BTreeSet<_>>>()?;
    ensure!(
        tags.iter()
            .all(|tag| matches!(*tag, "str" | "bool" | "int" | "float")),
        "config has an unsupported coercion type"
    );
    for tag in ["str", "bool", "int", "float"] {
        let variant = match tag {
            "str" => "Str",
            "bool" => "Bool",
            "int" => "Int",
            "float" => "Float",
            other => bail!("unsupported config type {other}"),
        };
        out.push_str(&format!(
            "    /// Python `{tag}` coercion.\n    {variant},\n"
        ));
    }
    out.push_str("}\n\n/// Settable configuration keys and their coercion types.\n#[doc(alias = \"bittensor.config.SETTABLE\")]\npub const SETTABLE: &[(&str, BuiltinType)] = &[\n");
    for (name, kind) in entries {
        let variant = match kind["type"].as_str().context("config type tag missing")? {
            "str" => "Str",
            "bool" => "Bool",
            "int" => "Int",
            "float" => "Float",
            other => bail!("unsupported config type {other}"),
        };
        out.push_str(&format!(
            "    ({}, BuiltinType::{variant}),\n",
            rust_string(&name)
        ));
    }
    out.push_str("];\n\n");
    for name in [
        "DEFAULT_CONFIG_PATH",
        "DEFAULT_PROXIES_PATH",
        "DEFAULT_ADDRESSES_PATH",
        "DEFAULT_MULTISIGS_PATH",
        "DEFAULT_MULTISIG_CACHE_PATH",
        "DEFAULT_SUBNET_NAMES_PATH",
        "DEFAULT_TOKEN_SYMBOLS_PATH",
    ] {
        let segments = value(config, name)?["home_relative"]
            .as_array()
            .context("config path must be home-relative")?
            .iter()
            .map(|segment| {
                segment
                    .as_str()
                    .map(rust_string)
                    .context("path component must be string")
            })
            .collect::<Result<Vec<_>>>()?
            .join(", ");
        out.push_str(&format!("/// Python `{name}`, relative to the user's home.\n#[doc(alias = \"bittensor.config.{name}\")]\npub const {name}: &[&str] = &[{segments}];\n\n"));
    }
    for name in ["SUBNET_NAMES_TTL_SECONDS", "TOKEN_SYMBOLS_TTL_SECONDS"] {
        let seconds = value(config, name)?
            .as_u64()
            .context("cache TTL must be u64")?;
        out.push_str(&format!(
            "/// Python `{name}` in seconds.\npub const {name}: u64 = {seconds};\n\n"
        ));
    }
    for (name, row) in environment
        .as_object()
        .context("env values must be an object")?
    {
        out.push_str(&format!("/// Runtime override for Python `{}`; empty values use the default.\npub const {name}: &str = {};\n\n",row["default"].as_str().context("default missing")?,rust_string(row["value"].as_str().context("env name missing")?)));
    }
    Ok(out.into_bytes())
}

fn render_runtime(document: &Value) -> Result<Vec<u8>> {
    let mut out = header(document)?;
    out.push_str(
        "//! Runtime metadata fetch and disk-cache format constants extracted from Python.\n\n",
    );
    for (source, name) in [
        ("_V15_VERSION_HEX", "V15_VERSION_HEX"),
        ("_V15_MISSING_NEEDLE", "V15_MISSING_NEEDLE"),
    ] {
        let text = value(document, source)?
            .as_str()
            .context("runtime string constant missing")?;
        out.push_str(&format!("/// Python `{source}`.\n#[doc(alias = \"bittensor._transport.runtime.{source}\")]\npub const {name}: &str = {};\n\n",rust_string(text)));
    }
    let magic = value(document, "_DISK_MAGIC")?["bytes_hex"]
        .as_str()
        .context("runtime magic must be bytes")?;
    ensure!(
        magic.len() % 2 == 0,
        "runtime magic has incomplete hex byte"
    );
    let bytes = magic
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| Ok(u8::from_str_radix(std::str::from_utf8(pair)?, 16)?.to_string()))
        .collect::<Result<Vec<_>>>()?;
    ensure!(!bytes.is_empty(), "runtime disk magic must not be empty");
    out.push_str(&format!("/// Python `_DISK_MAGIC` cache header bytes.\n#[doc(alias = \"bittensor._transport.runtime._DISK_MAGIC\")]\npub const DISK_MAGIC: &[u8; {}] = &[{}];\n\n",bytes.len(),bytes.join(", ")));
    let format = u32::try_from(
        value(document, "_DISK_FORMAT_VERSION")?
            .as_u64()
            .context("disk version must be u32")?,
    )?;
    let maximum = usize::try_from(
        value(document, "_DISK_CACHE_MAX_FILES")?
            .as_u64()
            .context("disk cache maximum must be usize")?,
    )?;
    out.push_str(&format!("/// Python `_DISK_FORMAT_VERSION`, used by cache filenames.\n#[doc(alias = \"bittensor._transport.runtime._DISK_FORMAT_VERSION\")]\npub const DISK_FORMAT_VERSION: u32 = {format};\n\n/// Python `_DISK_CACHE_MAX_FILES`.\n#[doc(alias = \"bittensor._transport.runtime._DISK_CACHE_MAX_FILES\")]\npub const DISK_CACHE_MAX_FILES: usize = {maximum};\n"));
    Ok(out.into_bytes())
}

pub(super) fn outputs(
    root: &Path,
    pins: &DocumentMut,
    tables: &Value,
) -> Result<Vec<(&'static str, Vec<u8>)>> {
    let mut resolver = Resolver::new(root)?;
    let mut enumeration = enum_document(&resolver, pins)?;
    enumeration.sort_all_objects();
    let (mut catalog, catalog_provenance) = catalog(root, pins)?;
    catalog.sort_all_objects();
    let descriptions = super::descriptions::checked(&mut resolver, pins, &catalog)?;
    let mut rules = classifier_rules(&resolver)?;
    rules["validated_bindings"] = classifier_bindings(&mut resolver, &enumeration, tables)?;
    let source_config = tables[CONFIG_PY].clone();
    let mut env = config_environment(&resolver)?;
    env.sort_all_objects();
    let errors = render_errors(&enumeration, &catalog, tables, &rules)?;
    let config = render_config(&source_config, &env)?;
    let runtime = tables["sdk/python/bittensor/_transport/runtime.py"].clone();
    let runtime_rust = render_runtime(&runtime)?;
    let mut config_document = source_config;
    config_document["environment"] = env;
    Ok(vec![
        (
            "vendor/upstream/extracted/error_descriptions.json",
            pretty(&descriptions)?,
        ),
        (
            "vendor/upstream/extracted/errors.json",
            pretty(
                &json!({"format":1,"catalog":catalog_provenance,"enum":enumeration,"classification_rules":rules,"classification_sources":[tables[ERRORS_PY]["provenance"].clone(),tables[RESULT_PY]["provenance"].clone()]}),
            )?,
        ),
        (
            "vendor/upstream/extracted/config.json",
            pretty(&config_document)?,
        ),
        ("crates/rtao-catalog/src/errors.rs", super::rustfmt(errors)?),
        ("crates/rtao-catalog/src/config.rs", super::rustfmt(config)?),
        ("vendor/upstream/extracted/runtime.json", pretty(&runtime)?),
        (
            "crates/rtao-catalog/src/runtime_py.rs",
            super::rustfmt(runtime_rust)?,
        ),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> Result<std::path::PathBuf> {
        Ok(Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .context("workspace root missing")?
            .to_path_buf())
    }

    #[test]
    fn error_catalog_rejects_semantic_and_chain_map_mismatches() -> Result<()> {
        let root = root()?;
        let pins = fs::read_to_string(root.join("upstream.toml"))?.parse::<DocumentMut>()?;
        let resolver = Resolver::new(&root)?;
        let enumeration = enum_document(&resolver, &pins)?;
        let (catalog, _) = catalog(&root, &pins)?;
        let rules = classifier_rules(&resolver)?;
        let corpus: Value = serde_json::from_slice(&fs::read(
            root.join("vendor/upstream/extracted/named_tables.json"),
        )?)?;
        render_errors(&enumeration, &catalog, &corpus["tables"], &rules)?;
        let mut changed = catalog.clone();
        changed["codes"]
            .as_object_mut()
            .context("codes missing")?
            .remove("unknown");
        assert!(render_errors(&enumeration, &changed, &corpus["tables"], &rules).is_err());
        let mut changed = catalog;
        changed["chain_errors"]["SlippageTooHigh"]["code"] = json!("not_found");
        assert!(render_errors(&enumeration, &changed, &corpus["tables"], &rules).is_err());
        Ok(())
    }

    #[test]
    fn classifier_rules_reject_changed_returns_and_follow_rate_needles() -> Result<()> {
        let root = root()?;
        let pins = fs::read_to_string(root.join("upstream.toml"))?.parse::<DocumentMut>()?;
        let mut resolver = Resolver::new(&root)?;
        let source = resolver.source(RESULT_PY)?;
        for changed in [
            source.replace("return _NAME_TO_CODE[name]", "return ErrorCode.UNKNOWN"),
            source.replace("return _DISPATCH_ERRORS[name][0]", "return _DISPATCH_ERRORS[name][1]"),
            source.replace("for needle, code in _SUBSTRING_FALLBACK:", "for code, needle in _SUBSTRING_FALLBACK:"),
            source.replace("if needle in haystack:\n            return code", "if needle in haystack:\n            return code\n        else:\n            return ErrorCode.UNKNOWN"),
        ] {
            resolver.sources.insert(RESULT_PY.into(), changed);
            assert!(classifier_rules(&resolver).is_err());
        }
        resolver.sources.insert(
            RESULT_PY.into(),
            source.replace(
                "name.endswith(\"RateLimitExceeded\") or \"RateLimit\" in name",
                "name.endswith(\"BurstExceeded\") or \"Burst\" in name",
            ),
        );
        let rules = classifier_rules(&resolver)?;
        assert_eq!(rules["rate_limit_suffix"], json!("BurstExceeded"));
        assert_eq!(rules["rate_limit_contains"], json!("Burst"));
        let enumeration = enum_document(&resolver, &pins)?;
        let (catalog, _) = catalog(&root, &pins)?;
        let corpus: Value = serde_json::from_slice(&fs::read(
            root.join("vendor/upstream/extracted/named_tables.json"),
        )?)?;
        let rendered = String::from_utf8(render_errors(
            &enumeration,
            &catalog,
            &corpus["tables"],
            &rules,
        )?)?;
        assert!(rendered.contains("name.ends_with(\"BurstExceeded\") || name.contains(\"Burst\")"));
        Ok(())
    }

    #[test]
    fn classifier_bindings_reject_replaced_imports_and_final_rebindings() -> Result<()> {
        let root = root()?;
        let pins = fs::read_to_string(root.join("upstream.toml"))?.parse::<DocumentMut>()?;
        let mut resolver = Resolver::new(&root)?;
        let source = resolver.source(RESULT_PY)?;
        let enumeration = enum_document(&resolver, &pins)?;
        let corpus: Value = serde_json::from_slice(&fs::read(
            root.join("vendor/upstream/extracted/named_tables.json"),
        )?)?;
        classifier_bindings(&mut resolver, &enumeration, &corpus["tables"])?;
        for changed in [
            source.replace(
                "import NAME_TO_CODE as _NAME_TO_CODE",
                "import DISPATCH_ERRORS as _NAME_TO_CODE",
            ),
            format!("{source}\n_NAME_TO_CODE = {{}}\n"),
            format!("{source}\nif dynamic_condition:\n    _DISPATCH_ERRORS = {{}}\n"),
            format!("{source}\nErrorCode = {{}}\n"),
        ] {
            let mut resolver = Resolver::new(&root)?;
            resolver.sources.insert(RESULT_PY.into(), changed);
            assert!(classifier_bindings(&mut resolver, &enumeration, &corpus["tables"]).is_err());
        }
        Ok(())
    }

    #[test]
    fn config_override_helpers_reject_changed_default_or_empty_semantics() -> Result<()> {
        let root = root()?;
        let mut resolver = Resolver::new(&root)?;
        let source = resolver.source(CONFIG_PY)?;
        let environment = config_environment(&resolver)?;
        assert_eq!(
            environment["CONFIG_ENV_VAR"]["value"],
            json!("BTCLI_CONFIG")
        );
        assert_eq!(
            environment["TOKEN_SYMBOLS_ENV_VAR"]["default"],
            json!("DEFAULT_TOKEN_SYMBOLS_PATH")
        );
        assert_eq!(environment["CONFIG_ENV_VAR"]["empty_is_unset"], json!(true));
        for changed in [
            source.replace(
                "os.getenv(\"BTCLI_CONFIG\") or DEFAULT_CONFIG_PATH",
                "os.environ.get(\"BTCLI_CONFIG\", DEFAULT_CONFIG_PATH)",
            ),
            source.replace(
                "os.getenv(\"BTCLI_CONFIG\") or DEFAULT_CONFIG_PATH",
                "os.getenv(\"BTCLI_CONFIG\") or DEFAULT_ADDRESSES_PATH",
            ),
        ] {
            resolver.sources.insert(CONFIG_PY.into(), changed);
            assert!(config_environment(&resolver).is_err());
        }
        Ok(())
    }

    #[test]
    fn runtime_wire_constants_preserve_string_bytes_and_checked_widths() -> Result<()> {
        let root = root()?;
        let pins = fs::read_to_string(root.join("upstream.toml"))?.parse::<DocumentMut>()?;
        let mut resolver = Resolver::new(&root)?;
        let runtime = super::super::document(
            &mut resolver,
            &pins,
            "sdk/python/bittensor/_transport/runtime.py",
            &[
                "_V15_VERSION_HEX",
                "_V15_MISSING_NEEDLE",
                "_DISK_MAGIC",
                "_DISK_FORMAT_VERSION",
                "_DISK_CACHE_MAX_FILES",
            ],
        )?;
        let rendered = String::from_utf8(render_runtime(&runtime)?)?;
        assert!(rendered.contains("V15_VERSION_HEX: &str = \"0x0f000000\""));
        assert!(rendered.contains("DISK_MAGIC: &[u8; 4] = &[83, 84, 77, 68]"));
        let mut changed = runtime;
        changed["values"]["_DISK_FORMAT_VERSION"]["value"] = json!(u64::MAX);
        assert!(render_runtime(&changed).is_err());
        Ok(())
    }
}
