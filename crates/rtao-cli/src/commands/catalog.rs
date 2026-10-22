// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/main.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Additional sources at the same commit/license: sdk/python/bittensor/intents/registry.py; sdk/python/bittensor/cli/output.py.
//! Full operation discovery and semantic or chain-error explanations.

use crate::{
    error::CliError,
    globals::{Tier, with_globals},
    output::{HumanView, Output, OutputError},
};
use clap::{Arg, ArgAction, ArgMatches, Command};
use rtao::tx::{ChainError, ToolSpec};
use rtao_catalog::{ErrorCode, errors};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::OnceLock,
};

/// Actual Rust interface metadata supplied by the compiled command registry.
///
/// Metadata is separate from the complete upstream catalog. Missing records
/// retain all upstream fields and have null Rust fields, so catalog discovery
/// never claims a command that this binary cannot run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RustTool {
    /// Exact upstream operation name.
    pub name: String,
    /// Public path of the generated interface type.
    pub type_name: String,
    /// Rust signing-role marker.
    pub role: String,
    /// Rust output type.
    pub output: String,
    /// An implemented command invocation, or `None` for unavailable commands.
    pub cli: Option<String>,
}

/// The always-JSON operation catalog command.
pub fn tools_command() -> Command {
    with_globals(
        Command::new("tools")
            .about("Print the machine-readable operation catalog (JSON) for agents."),
        Tier::Read,
    )
}

/// Error explanation and full chain-catalog command.
pub fn explain_command() -> Command {
    with_globals(
        Command::new("explain")
            .about("Long-form explanation of an error code (like `rustc --explain`).")
            .arg(Arg::new("code").help(
                "Error code from a diagnostic (e.g. insufficient_balance) or an exact chain error name (e.g. SettingWeightsTooFast). With --chain: a pallet filter."
            ))
            .arg(Arg::new("chain").long("chain").action(ArgAction::SetTrue).help("List every chain error with its code and description.")),
        Tier::Read,
    )
}

/// Project all 86 intents into Python's nine tool fields plus D-029's `rust`.
///
/// This mirrors [`rtao::tx::Executor::tools`]: deserialize the authenticated
/// catalog record into [`ToolSpec`] and normalize its documentation URL.
/// The `rust` object is additive and never changes an upstream field.
pub fn tools_payload(rust_tools: &[RustTool]) -> Result<Value, CliError> {
    let mut supplied = BTreeMap::new();
    for info in rust_tools {
        if !rtao_catalog::INTENTS
            .iter()
            .any(|spec| spec.name == info.name)
        {
            return Err(invalid_catalog(format!(
                "unknown Rust operation {:?}",
                info.name
            )));
        }
        if supplied.insert(info.name.as_str(), info).is_some() {
            return Err(invalid_catalog(format!(
                "duplicate Rust operation {:?}",
                info.name
            )));
        }
    }
    let mut records = Vec::with_capacity(rtao_catalog::INTENTS.len());
    for spec in &rtao_catalog::INTENTS {
        let mut tool: ToolSpec = serde_json::from_str(spec.catalog_json)
            .map_err(|error| invalid_catalog(error.to_string()))?;
        tool.docs_url = format!(
            "{}/tx/{}",
            rtao_catalog::py::DOCS_URL,
            spec.name.replace('_', "-")
        );
        let mut record =
            serde_json::to_value(tool).map_err(|error| invalid_catalog(error.to_string()))?;
        record["rust"] = match supplied.get(spec.name) {
            Some(info) => {
                json!({"type":info.type_name,"role":info.role,"output":info.output,"cli":info.cli})
            }
            None => json!({"type":null,"role":null,"output":null,"cli":null}),
        };
        records.push(record);
    }
    records.sort_by(|left, right| left["name"].as_str().cmp(&right["name"].as_str()));
    Ok(Value::Array(records))
}

/// Write catalog JSON to the supplied output sink even in human or quiet mode.
pub fn run_tools(output: &Output, rust_tools: &[RustTool]) -> Result<(), CliError> {
    let payload = tools_payload(rust_tools)?;
    let mut options = output.options().clone();
    options.json_mode = true;
    Output::with_sink(options, output.sink().clone())
        .value(&payload)
        .map_err(output_failure)
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ChainInfo {
    pallet: String,
    name: String,
    docs: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ErrorId {
    tuple: (u8, u8),
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ErrorConstructor {
    constructor: String,
    fields: ChainInfo,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ErrorTable {
    dict: Vec<(ErrorId, ErrorConstructor)>,
}

fn invalid_catalog(message: impl std::fmt::Display) -> CliError {
    CliError::failure(format!("authenticated CLI catalog is invalid: {message}"))
}
fn output_failure(error: OutputError) -> CliError {
    CliError::failure(error.to_string())
}

fn decode_errors(source: &str) -> Result<Vec<ChainInfo>, String> {
    let table: ErrorTable = serde_json::from_str(source).map_err(|error| error.to_string())?;
    let mut ids = BTreeSet::new();
    table
        .dict
        .into_iter()
        .map(|(id, entry)| {
            if entry.constructor != "ErrorInfo" || !ids.insert(id.tuple) {
                return Err("unexpected constructor or duplicate runtime error identity".into());
            }
            Ok(entry.fields)
        })
        .collect()
}

fn chain_errors() -> Result<&'static [ChainInfo], CliError> {
    static ERRORS: OnceLock<Result<Vec<ChainInfo>, String>> = OnceLock::new();
    match ERRORS.get_or_init(|| {
        let table = rtao_catalog::py::PYTHON_TABLES
            .iter()
            .find(|table| {
                table.path == "sdk/python/bittensor/_generated/errors.py" && table.name == "ERRORS"
            })
            .ok_or_else(|| "missing Python ERRORS declaration".to_owned())?;
        decode_errors(table.value_json)
    }) {
        Ok(errors) => Ok(errors),
        Err(error) => Err(invalid_catalog(error)),
    }
}

fn error_docs(code: ErrorCode) -> String {
    format!(
        "{}/errors/{}",
        rtao_catalog::py::DOCS_URL,
        code.as_str().replace('_', "-")
    )
}
fn description(name: &str) -> Option<&'static str> {
    rtao_catalog::CHAIN_ERRORS
        .iter()
        .find(|info| info.name == name)
        .map(|info| info.description)
}
fn chain_docs(name: &str) -> String {
    format!("{}/errors/chain/{name}", rtao_catalog::py::DOCS_URL)
}

/// Return the full runtime-error list in Python's metadata declaration order.
///
/// Dispatch-level errors are explained by name but do not enter this listing.
/// A pallet filter is case-insensitive and keeps every error for that pallet.
pub fn chain_catalog(pallet: Option<&str>) -> Result<Value, CliError> {
    let errors = chain_errors()?;
    let pallets: BTreeSet<_> = errors.iter().map(|info| info.pallet.as_str()).collect();
    if let Some(pallet) = pallet
        && !pallets
            .iter()
            .any(|name| name.to_lowercase() == pallet.to_lowercase())
    {
        return Err(CliError::failure(format!(
            "unknown pallet {}",
            rtao_rpc::policy::python_string_repr(pallet)
        ))
        .with_help(format!(
            "valid pallets: {}",
            pallets.into_iter().collect::<Vec<_>>().join(", ")
        )));
    }
    let records = errors
        .iter()
        .filter(|info| {
            pallet.is_none_or(|pallet| info.pallet.to_lowercase() == pallet.to_lowercase())
        })
        .map(|info| {
            json!({
                "pallet":info.pallet,
                "name":info.name,
                "code":errors::classify_error(&info.docs,Some(&info.name)).as_str(),
                "description":info.docs,
                "docs_url":description(&info.name).map(|_|chain_docs(&info.name)),
            })
        })
        .collect();
    Ok(Value::Array(records))
}

/// Explain an exact chain or dispatch variant, accepting pallet-qualified names.
///
/// Comparisons are case-insensitive; duplicate names retain one record for each
/// matching pallet. An unknown name returns an empty array.
pub fn chain_matches(query: &str) -> Result<Vec<Value>, CliError> {
    let (pallet, name) = query.rsplit_once('.').unwrap_or(("", query));
    let mut matches = Vec::new();
    for (dispatch_name, code) in errors::DISPATCH_ERRORS {
        if dispatch_name.to_lowercase() != name.to_lowercase()
            || (!pallet.is_empty() && pallet.to_lowercase() != "runtime")
        {
            continue;
        }
        let docs = rtao_catalog::py::DISPATCH_ERROR_DOCS
            .iter()
            .find_map(|(candidate, docs)| (*candidate == *dispatch_name).then_some(*docs))
            .ok_or_else(|| invalid_catalog(format!("missing dispatch docs for {dispatch_name}")))?;
        matches.push(json!({
            "pallet":"Runtime", "name":dispatch_name, "code":code.as_str(),
            "docs":docs,"description":docs,
            "help":ChainError::new("",Some((*dispatch_name).into())).remediation(),
            "docs_url":error_docs(*code),
        }));
    }
    for info in chain_errors()? {
        if info.name.to_lowercase() != name.to_lowercase()
            || (!pallet.is_empty() && info.pallet.to_lowercase() != pallet.to_lowercase())
        {
            continue;
        }
        let code = errors::classify_error(&info.docs, Some(&info.name));
        matches.push(json!({
            "pallet":info.pallet,"name":info.name,"code":code.as_str(),"docs":info.docs,
            "description":description(&info.name).unwrap_or(""),
            "help":ChainError::new("",Some(info.name.clone())).remediation(),
            "docs_url":if description(&info.name).is_some(){chain_docs(&info.name)}else{error_docs(code)},
        }));
    }
    Ok(matches)
}

/// Execute `explain` without a network connection or signing capability.
///
/// Diagnostics are returned for the application's single error renderer.
pub fn run_explain(output: &Output, matches: &ArgMatches) -> Result<(), CliError> {
    let code = matches.get_one::<String>("code").map(String::as_str);
    if matches.get_flag("chain") || code.is_none() {
        let payload = chain_catalog(code)?;
        emit(output, &payload, catalog_text(&payload))?;
        if code.is_none() && !matches.get_flag("chain") {
            output.message("explain one entry with `btcli explain <Name>` or `btcli explain <code>`; filter with `btcli explain --chain <pallet>`").map_err(output_failure)?;
        }
        return Ok(());
    }
    let query = crate::prompt::python_strip(code.unwrap_or_default());
    if let Some(code) = ErrorCode::from_code(&query.to_lowercase()) {
        let spec = rtao_catalog::SEMANTIC_ERRORS
            .iter()
            .find(|info| info.code == code)
            .ok_or_else(|| invalid_catalog(format!("missing semantic docs for {code}")))?;
        let payload = json!({"code":code.as_str(),"explanation":spec.explanation,"help":spec.remediation,"see":error_docs(code)});
        return emit(
            output,
            &payload,
            format!(
                "error[{code}]\n\n{}\n\n  help: {}\n  see: {}",
                spec.explanation,
                spec.remediation,
                error_docs(code)
            ),
        );
    }
    let matches = chain_matches(query)?;
    if matches.is_empty() {
        return Err(CliError::failure(format!("unknown error code {}",rtao_rpc::policy::python_string_repr(query)))
            .with_help("run `btcli explain` for the semantic codes, or `btcli explain --chain` for the chain error catalog"));
    }
    let mut text = matches
        .iter()
        .map(|record| {
            let body = record["description"]
                .as_str()
                .filter(|text| !text.is_empty())
                .or_else(|| record["docs"].as_str().filter(|text| !text.is_empty()))
                .unwrap_or_else(|| record["name"].as_str().unwrap_or_default());
            format!(
                "error[{}]: {}.{}\n\n{body}\n\n  help: {}\n  see: {}",
                field(record, "code"),
                field(record, "pallet"),
                field(record, "name"),
                field(record, "help"),
                field(record, "docs_url")
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    let mut codes = BTreeSet::new();
    for record in &matches {
        let code = field(record, "code");
        if codes.insert(code) {
            text.push_str(&format!(
                "\nfor more information about this code, run `btcli explain {code}`"
            ));
        }
    }
    let payload = if matches.len() == 1 {
        matches.into_iter().next().unwrap_or(Value::Null)
    } else {
        Value::Array(matches)
    };
    emit(output, &payload, text)
}

fn field<'a>(record: &'a Value, name: &str) -> &'a str {
    record[name].as_str().unwrap_or_default()
}
fn catalog_text(payload: &Value) -> String {
    let mut text = String::from("chain errors\n");
    let Some(records) = payload.as_array() else {
        return text;
    };
    let width = records
        .iter()
        .map(|record| field(record, "name").len())
        .max()
        .unwrap_or(0);
    let mut pallet = "";
    for record in records {
        let next = field(record, "pallet");
        if next != pallet {
            text.push_str(&format!("\n{next}\n"));
            pallet = next;
        }
        text.push_str(&format!(
            "  {:width$}  {}\n    {}\n",
            field(record, "name"),
            field(record, "code"),
            field(record, "description")
        ));
    }
    text.push_str(&format!(
        "\n{} {}",
        records.len(),
        if records.len() == 1 {
            "error"
        } else {
            "errors"
        }
    ));
    text
}
fn emit(output: &Output, payload: &Value, human: String) -> Result<(), CliError> {
    let mut rendered = output.clone();
    rendered
        .registry_mut()
        .register("catalog-explain", move |_| {
            Ok(HumanView::Lines(
                human.split('\n').map(str::to_owned).collect(),
            ))
        });
    rendered
        .render("catalog-explain", payload)
        .map_err(output_failure)
}
