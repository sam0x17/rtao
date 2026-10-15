//! Typed execution constants and a lossless index of all resolved Python tables.

use anyhow::{Context, Result, ensure};
use serde_json::Value;

use super::{header, rust_string, value};

fn declaration(tables: &Value, path: &str, name: &str) -> Result<Value> {
    let table = tables
        .get(path)
        .with_context(|| format!("Python table module {path} missing"))?;
    Ok(
        serde_json::json!({"provenance":table["provenance"],"name":name,"source":table["values"][name],"value":value(table,name)?}),
    )
}

pub(super) fn render(tables: &Value) -> Result<String> {
    let mut out = String::new();
    let module_errors = declaration(
        tables,
        "sdk/python/bittensor/_generated/errors.py",
        "ERRORS",
    )?;
    out.push_str(&header(&module_errors)?);
    out.push_str("/// Generated metadata error docs in declaration order, retaining duplicate names and empty docs.\npub const MODULE_ERROR_DOCS: &[(&str,&str)] = &[\n");
    for pair in module_errors["value"]["dict"]
        .as_array()
        .context("metadata error dictionary absent")?
    {
        let fields = &pair[1]["fields"];
        out.push_str(&format!(
            "({},{}),\n",
            rust_string(
                fields["name"]
                    .as_str()
                    .context("module error name absent")?
            ),
            rust_string(
                fields["docs"]
                    .as_str()
                    .context("module error docs absent")?
            )
        ));
    }
    out.push_str("];\n\n");
    let dispatch = declaration(
        tables,
        "sdk/python/bittensor/error_map.py",
        "DISPATCH_ERRORS",
    )?;
    out.push_str(&header(&dispatch)?);
    out.push_str("/// Exact short Python dispatch error meanings in declaration order.\npub const DISPATCH_ERROR_DOCS: &[(&str,&str)] = &[\n");
    for pair in dispatch["value"]["dict"]
        .as_array()
        .context("dispatch error dictionary absent")?
    {
        out.push_str(&format!(
            "({},{}),\n",
            rust_string(pair[0].as_str().context("dispatch name absent")?),
            rust_string(
                pair[1]["tuple"][1]
                    .as_str()
                    .context("dispatch docs absent")?
            )
        ));
    }
    out.push_str("];\n\n");
    for (path, source, name) in [
        (
            "sdk/python/bittensor/executor.py",
            "_MAX_SHIELDED_CIPHERTEXT_BYTES",
            "MAX_SHIELDED_CIPHERTEXT_BYTES",
        ),
        (
            "sdk/python/bittensor/executor.py",
            "_SHIELDED_POLL_ATTEMPTS_PER_BLOCK",
            "SHIELDED_POLL_ATTEMPTS_PER_BLOCK",
        ),
        (
            "sdk/python/bittensor/executor.py",
            "_SHIELDED_FINALITY_RPC_RETRIES",
            "SHIELDED_FINALITY_RPC_RETRIES",
        ),
        (
            "sdk/python/bittensor/multisig.py",
            "_FEE_ALLOWANCE_RAO",
            "MULTISIG_FEE_ALLOWANCE_RAO",
        ),
    ] {
        let row = declaration(tables, path, source)?;
        out.push_str(&header(&row)?);
        let number = row["value"]
            .as_u64()
            .context("execution bound must fit u64")?;
        out.push_str(&format!("/// Python `{source}`.\n#[doc(alias = \"bittensor.{}.{source}\")]\npub const {name}: u64 = {number};\n\n",if path.contains("executor") {"executor"} else {"multisig"}));
    }
    let transient = declaration(
        tables,
        "sdk/python/bittensor/executor.py",
        "_TRANSIENT_SUBSTRINGS",
    )?;
    out.push_str(&header(&transient)?);
    out.push_str("/// Ordered Python transaction retry needles.\npub const TRANSIENT_SUBSTRINGS: &[&str] = &[\n");
    for value in transient["value"]["tuple"]
        .as_array()
        .context("transient needles must be tuple")?
    {
        out.push_str(&format!(
            "{},\n",
            rust_string(value.as_str().context("retry needle must be string")?)
        ));
    }
    out.push_str("];\n\n");
    for (source, name) in [
        ("_CUSTOM_ERROR_RE", "CUSTOM_ERROR_RE"),
        ("_WATCH_STATUS_RE", "WATCH_STATUS_RE"),
    ] {
        let row = declaration(tables, "sdk/python/bittensor/result.py", source)?;
        out.push_str(&header(&row)?);
        ensure!(
            row["value"]["flags"]["enum"].as_str() == Some("re.IGNORECASE"),
            "result regex flags changed"
        );
        out.push_str(&format!("/// Exact Python `{source}` pattern, interpreted case-insensitively.\npub const {name}: &str = {};\n\n",rust_string(row["value"]["regex"].as_str().context("result regex pattern missing")?)));
    }
    for (source, name) in [
        ("_WATCH_STATUS_NAMES", "WATCH_STATUS_NAMES"),
        ("_NAME_HELP_OVERRIDES", "NAME_HELP_OVERRIDES"),
    ] {
        let row = declaration(tables, "sdk/python/bittensor/result.py", source)?;
        out.push_str(&header(&row)?);
        out.push_str(&format!(
            "/// Exact ordered Python `{source}`.\npub const {name}: &[(&str,&str)] = &[\n"
        ));
        for pair in row["value"]["dict"]
            .as_array()
            .context("result table must be dictionary")?
        {
            out.push_str(&format!(
                "({},{}),\n",
                rust_string(pair[0].as_str().context("result key must be string")?),
                rust_string(pair[1].as_str().context("result value must be string")?)
            ));
        }
        out.push_str("];\n\n");
    }
    let overrides = declaration(tables, "sdk/python/bittensor/result.py", "_HELP_OVERRIDES")?;
    out.push_str(&header(&overrides)?);
    out.push_str("/// Ordered Python display-text help overrides.\npub const HELP_OVERRIDES: &[(&str,&str)] = &[\n");
    for pair in overrides["value"]["tuple"]
        .as_array()
        .context("help overrides must be tuple")?
    {
        out.push_str(&format!(
            "({},{}),\n",
            rust_string(pair["tuple"][0].as_str().context("help needle missing")?),
            rust_string(pair["tuple"][1].as_str().context("help text missing")?)
        ));
    }
    out.push_str("];\n\n");
    for (source, name) in [
        ("REMEDIATION", "REMEDIATION"),
        ("EXPLANATIONS", "EXPLANATIONS"),
    ] {
        let row = declaration(tables, "sdk/python/bittensor/result.py", source)?;
        out.push_str(&header(&row)?);
        out.push_str(&format!("/// Exact ordered Python semantic-code `{source}`.\npub const {name}: &[(crate::ErrorCode,&str)] = &[\n"));
        for pair in row["value"]["dict"]
            .as_array()
            .context("semantic documentation must be dictionary")?
        {
            let token = pair[0].as_str().context("semantic token missing")?;
            let variant = super::catalog::variant(&token.to_ascii_uppercase())?;
            out.push_str(&format!(
                "(crate::ErrorCode::{variant},{}),\n",
                rust_string(pair[1].as_str().context("semantic text missing")?)
            ));
        }
        out.push_str("];\n\n");
    }
    let warning = declaration(
        tables,
        "sdk/python/bittensor/fee_filters.py",
        "COLDKEY_FEE_WARNING",
    )?;
    out.push_str(&header(&warning)?);
    out.push_str(&format!("/// Exact Python coldkey fee-routing warning.\npub const COLDKEY_FEE_WARNING: &str = {};\n\n",rust_string(warning["value"].as_str().context("fee warning missing")?)));
    out.push_str("/// Chain classification and transaction rejection tables shared with Python.\npub use crate::errors::{NAME_TO_CODE,CUSTOM_TRANSACTION_ERRORS,INVALID_TRANSACTION_ERRORS,DISPATCH_ERRORS,SUBSTRING_FALLBACK};\n\n/// Python's fee-routing mirror, checked against the authoritative runtime.\npub use crate::rt::COLDKEY_PAYS_FEE_CALLS;\n\n/// A lossless resolved Python declaration, including exact integer/container tags.\n#[derive(Clone, Copy, Debug, Eq, PartialEq)]\npub struct PythonTable {\n/// Monorepo source path.\npub path: &'static str,\n/// Python declaration name.\npub name: &'static str,\n/// Exact canonical extractor representation.\npub value_json: &'static str,\n/// Authenticated Git blob identity.\npub blob: &'static str,\n/// Declaration start line.\npub line: u32,\n/// Declaration end line.\npub end_line: u32,\n}\n\n/// All named Python constants, intent defaults and class variables.\npub static PYTHON_TABLES: &[PythonTable] = &[\n");
    for (path, table) in tables.as_object().context("Python module corpus missing")? {
        let blob = table["provenance"]["blob"]
            .as_str()
            .context("Python source blob missing")?;
        for (name, row) in table["values"]
            .as_object()
            .context("Python value map missing")?
        {
            let mut value = row["value"].clone();
            value.sort_all_objects();
            let line = u32::try_from(row["line"].as_u64().context("Python source line missing")?)?;
            let end = u32::try_from(
                row["end_line"]
                    .as_u64()
                    .context("Python source end line missing")?,
            )?;
            out.push_str(&format!("PythonTable {{path:{},name:{},value_json:{},blob:{},line:{line},end_line:{end}}},\n",rust_string(path),rust_string(name),rust_string(&serde_json::to_string(&value)?),rust_string(blob)));
        }
    }
    out.push_str("];\n\n/// Find a resolved declaration by authenticated source path and Python name.\npub fn table(path: &str,name: &str) -> Option<&'static PythonTable> { PYTHON_TABLES.iter().find(|row|row.path==path&&row.name==name) }\n");
    Ok(out)
}
