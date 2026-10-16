// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/tx.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/query.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/context.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Argument shapes and coercion for generated commands; money stays exact.

use crate::{
    books::{address_cli_name, ss58_param_help},
    error::CliError,
    prompt::python_strip,
};
use clap::{Arg, ArgAction, ArgMatches, Command, parser::ValueSource};
use rtao::{
    primitives::{DynBalance, Money, NetUid},
    reads::ReadName,
};
use rtao_catalog::{IntentSpec, ParamSpec};
use serde_json::{Map, Value, json};

/// One authenticated argument's command-line shape.
#[derive(Clone, Debug)]
pub struct Field {
    /// Original Python argument name, also reserved from globals.
    pub name: &'static str,
    /// Original annotation used to distinguish list and numeric coercion.
    pub annotation: &'static str,
    /// JSON type from the input schema or read registration.
    pub kind: String,
    /// Missing required fields must be prompted or refused with exit2.
    pub required: bool,
    /// Whether an omitted own-key value may resolve through the wallet.
    pub own_key: bool,
    /// Exact authenticated default, including null.
    pub default: Option<Value>,
    /// Whether the field permits the chain-resolved all sentinel.
    pub allow_all: bool,
    /// Source documentation with the input-shape notes.
    pub help: String,
    /// Query arrays are CSV; transaction arrays also accept JSON.
    pub query: bool,
}

fn base(annotation: &str) -> &str {
    annotation
        .strip_prefix("Optional[")
        .and_then(|s| s.strip_suffix(']'))
        .or_else(|| annotation.strip_suffix(" | None"))
        .unwrap_or(annotation)
}
impl Field {
    /// Source flag spelling, with `_ss58` removed from account arguments.
    pub fn flag(&self) -> String {
        if self.name.contains("_ss58") {
            address_cli_name(self.name)
        } else {
            format!("--{}", self.name.replace('_', "-"))
        }
    }
    /// Whether the scalar is a Python bool, including an optional bool.
    pub fn boolean(&self) -> bool {
        self.kind == "boolean" || base(self.annotation) == "bool"
    }
    /// Whether an omitted own-key argument must preserve its explicit null default.
    pub fn preserve_null(&self) -> bool {
        self.query && self.default.as_ref().is_some_and(Value::is_null)
    }
}

fn field(
    param: &'static ParamSpec,
    property: Option<&Value>,
    allow_all: bool,
    query: bool,
) -> Result<Field, CliError> {
    let annotation = param.annotation.unwrap_or("str");
    let annotation_base = base(annotation);
    let mut kind = param
        .json_type
        .or_else(|| property.and_then(|v| v.get("type")).and_then(Value::as_str))
        .unwrap_or_else(|| match annotation_base {
            "bool" => "boolean",
            "int" => "integer",
            "float" => "number",
            "dict" => "object",
            value if value == "list" || value.starts_with("list[") => "array",
            _ => "string",
        })
        .to_owned();
    if !query {
        if annotation_base == "dict" || annotation_base.starts_with("dict[") {
            kind = "json".into();
        } else if annotation_base == "list" || annotation_base.starts_with("list[") {
            kind = "array".into();
        } else if annotation_base == "int | float | str" {
            kind = "string".into();
        }
    }
    let mut help = property
        .and_then(|v| v.get("description"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    if param.name.ends_with("_ss58") || param.name.ends_with("_ss58s") {
        let scalar = if param.name.ends_with("_ss58s") {
            param.name.strip_suffix('s').unwrap_or(param.name)
        } else {
            param.name
        };
        let mut note = ss58_param_help(scalar);
        if (query && param.default_json.is_some()) || param.name.ends_with("_ss58s") {
            note = note
                .split(" Defaults to your wallet")
                .next()
                .unwrap_or(&note)
                .to_owned();
        }
        if kind == "array" {
            note.push_str(" Comma-separated account references.");
        }
        help.push_str(&format!(" {note}"));
    } else if annotation_base == "Money" {
        help.push_str(if param.name.ends_with("_alpha") {
            " Amount in subnet alpha (TAO for netuid0)."
        } else {
            " Amount in TAO."
        });
    } else if kind == "array" {
        help.push_str(if query {
            " Comma-separated list."
        } else {
            " Comma-separated values or a JSON list."
        });
    }
    if allow_all {
        help.push_str(" Pass all for the entire available amount.");
    }
    let own_key = param.own_key || matches!(param.name, "hotkey_ss58" | "coldkey_ss58");
    let required = param.required && !own_key;
    if required {
        help.push_str(" (required)");
    }
    Ok(Field {
        name: param.name,
        annotation,
        kind,
        required,
        own_key,
        default: param
            .default_json
            .map(serde_json::from_str)
            .transpose()
            .map_err(|error| {
                CliError::failure(format!("invalid authenticated default: {error}"))
            })?,
        allow_all,
        help: help.trim().into(),
        query,
    })
}

/// Extract transaction arguments from the unchanged authenticated catalog schema.
pub fn intent_fields(spec: &'static IntentSpec) -> Result<Vec<Field>, CliError> {
    let schema: Value =
        serde_json::from_str(spec.catalog_json).map_err(|e| CliError::failure(e.to_string()))?;
    let native = rtao_intents::interface_metadata()
        .into_iter()
        .find(|row| row.name == spec.name);
    spec.fields
        .iter()
        .map(|p| {
            let mut result = field(
                p,
                schema["input_schema"]["properties"].get(p.name),
                spec.all_amount_fields.contains(&p.name),
                false,
            )?;
            if native
                .as_ref()
                .is_some_and(|row| row.json_number_fields.contains(&p.name))
            {
                result.kind = "native_number".into();
            }
            Ok(result)
        })
        .collect()
}
/// Extract read arguments, including exact signature defaults and own-key policy.
pub fn read_fields(name: ReadName) -> Result<Vec<Field>, CliError> {
    let spec = name.catalog_spec();
    let catalog: Value =
        serde_json::from_str(spec.catalog_json).map_err(|e| CliError::failure(e.to_string()))?;
    spec.params
        .iter()
        .map(|p| {
            let mut result = field(p, None, false, true)?;
            if let Some(doc) = catalog["param_docs"].get(p.name).and_then(Value::as_str) {
                result.help = format!("{doc} {}", result.help);
            }
            Ok(result)
        })
        .collect()
}

/// Install source-shaped options. Required values reach the body for typed prompts.
/// This mirrors Python's optional Click layer and preserves richer address pickers.
pub fn arguments(mut command: Command, fields: &[Field], tx: bool) -> Command {
    let amount_fields = fields
        .iter()
        .filter(|f| matches!(f.name, "amount_tao" | "amount_alpha"))
        .count();
    for field in fields {
        let mut arg = Arg::new(field.name)
            .long(field.flag().trim_start_matches("--").to_owned())
            .help(field.help.clone());
        if tx && amount_fields == 1 && matches!(field.name, "amount_tao" | "amount_alpha") {
            arg = arg.alias("amount");
        }
        if tx && field.name == "dest_coldkey_ss58" && !fields.iter().any(|f| f.name == "dest_ss58")
        {
            arg = arg.alias("dest");
        }
        if field.boolean() {
            let opposite = format!("__no_{}", field.name);
            arg = arg
                .action(ArgAction::SetTrue)
                .overrides_with(opposite.clone());
            command = command.arg(
                Arg::new(opposite)
                    .long(format!("no-{}", field.flag().trim_start_matches("--")))
                    .action(ArgAction::SetTrue)
                    .overrides_with(field.name)
                    .help(format!("Disable {}.", field.flag())),
            );
        } else {
            if matches!(field.kind.as_str(), "integer" | "number" | "native_number")
                || base(field.annotation) == "int | float | str"
            {
                arg = arg.allow_negative_numbers(true);
            }
            let parser = field.clone();
            arg = arg.value_parser(clap::builder::ValueParser::new(
                move |raw: &str| -> Result<String, String> {
                    coerce(&parser, raw)
                        .map(|_| raw.to_owned())
                        .map_err(|e| e.message)
                },
            ));
        }
        command = command.arg(arg);
    }
    command
}

/// Read supplied options, retaining false/zero/empty strings and real defaults.
pub fn values(matches: &ArgMatches, fields: &[Field]) -> Result<Map<String, Value>, CliError> {
    let mut result = Map::new();
    for field in fields {
        let value = if field.boolean() {
            let opposite = format!("__no_{}", field.name);
            if matches.value_source(field.name) == Some(ValueSource::CommandLine) {
                Some(json!(true))
            } else if matches.value_source(&opposite) == Some(ValueSource::CommandLine) {
                Some(json!(false))
            } else {
                field.default.clone()
            }
        } else {
            matches
                .get_one::<String>(field.name)
                .map(|raw| coerce(field, raw))
                .transpose()?
                .or_else(|| field.default.clone())
        };
        if let Some(value) = value {
            result.insert(field.name.into(), value);
        }
    }
    Ok(result)
}

fn integer(raw: &str) -> Result<Value, CliError> {
    let integer = rtao::primitives::pyfmt::parse_integer(raw)
        .ok_or_else(|| CliError::input(format!("expected an integer, got {raw:?}")))?;
    // The generated native parameter type enforces its exact range afterwards.
    rtao::primitives::json::from_str(&integer.to_string())
        .map_err(|error| CliError::input(error.to_string()))
}
fn number(raw: &str) -> Result<Value, CliError> {
    let number = rtao::primitives::pyfmt::parse_float(raw)
        .filter(|value| value.is_finite())
        .ok_or_else(|| CliError::input(format!("expected a finite number, got {raw:?}")))?;
    Ok(json!(number))
}

const MONEY_TEXT_BUDGET: usize = 4096;

/// Normalize Decimal-compatible CLI text while keeping Money/Amount strict.
/// Text and exponent budgets also bound Unicode and redundant-zero scratch work.
/// Callers enforce whether their field permits the returned all sentinel.
pub(super) fn normalize_money_text(raw: &str, netuid: NetUid) -> Result<String, CliError> {
    const EXPONENT_BUDGET: u64 = 4096;
    if raw.len() > MONEY_TEXT_BUDGET {
        return Err(CliError::input(
            "amount exceeds its 4096-byte normalization budget",
        ));
    }
    let text = crate::prompt::python_strip(raw);
    if text.eq_ignore_ascii_case("all") {
        return Ok("all".into());
    }
    let invalid = || CliError::input("amount must be a finite non-negative decimal or 'all'");
    // Decimal(string) ignores underscores and accepts Unicode decimal digits.
    let text = text
        .chars()
        .filter(|character| *character != '_')
        .collect::<String>();
    let (negative, text) = match text.strip_prefix('-') {
        Some(unsigned) => (true, unsigned),
        None => (false, text.strip_prefix('+').unwrap_or(&text)),
    };
    let decimal_digits = |value: &str| {
        value.chars().all(|character| {
            unicode_general_category::get_general_category(character)
                == unicode_general_category::GeneralCategory::DecimalNumber
        })
    };
    let (mantissa, exponent) = match text.find(['e', 'E']) {
        Some(index) => {
            let (mantissa, suffix) = text.split_at(index);
            let exponent = &suffix[1..];
            let unsigned = exponent.strip_prefix(['+', '-']).unwrap_or(exponent);
            if unsigned.is_empty() || !decimal_digits(unsigned) {
                return Err(invalid());
            }
            let exponent = rtao::primitives::pyfmt::parse_integer(exponent)
                .ok_or_else(invalid)?
                .to_string()
                .parse::<i64>()
                .map_err(|_| CliError::input("amount exponent exceeds its normalization budget"))?;
            if exponent.unsigned_abs() > EXPONENT_BUDGET {
                return Err(CliError::input(
                    "amount exponent exceeds its normalization budget",
                ));
            }
            (mantissa, exponent)
        }
        None => (text, 0),
    };
    let (integer, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    if (integer.is_empty() && fraction.is_empty())
        || !decimal_digits(integer)
        || !decimal_digits(fraction)
    {
        return Err(invalid());
    }
    let coefficient = rtao::primitives::pyfmt::parse_integer(&format!("{integer}{fraction}"))
        .ok_or_else(invalid)?
        .to_string();
    if negative && coefficient != "0" {
        return Err(CliError::input("amount must be non-negative"));
    }
    // Remove redundant zero precision before the exact native constructor.
    // This admits e.g. 10e-10 as one rao without admitting 1e-10 or rounding.
    let significant = coefficient.trim_end_matches('0');
    let (significant, power) = if significant.is_empty() {
        ("0", 0)
    } else {
        let fraction_digits = i64::try_from(fraction.chars().count())
            .map_err(|error| CliError::failure(error.to_string()))?;
        let trailing_zeros = i64::try_from(coefficient.len() - significant.len())
            .map_err(|error| CliError::failure(error.to_string()))?;
        let power = exponent
            .checked_sub(fraction_digits)
            .and_then(|power| power.checked_add(trailing_zeros))
            .ok_or_else(|| CliError::input("amount exponent exceeds its normalization budget"))?;
        (significant, power)
    };
    let balance = DynBalance::from_amount(format!("{significant}e{power}"), netuid)
        .map_err(|error| CliError::input(error.to_string()))?;
    Ok(Money::from_rao(balance.rao).to_decimal_string())
}

/// Coerce one scalar/list/object without a floating-point conversion for money.
pub fn coerce(field: &Field, raw: &str) -> Result<Value, CliError> {
    let annotation = base(field.annotation);
    if annotation == "Money" {
        let text = python_strip(raw);
        if text.is_empty() && field.allow_all && raw.len() <= MONEY_TEXT_BUDGET {
            return Ok(json!(""));
        }
        let amount = normalize_money_text(raw, NetUid::ROOT)?;
        if amount == "all" && !field.allow_all {
            return Err(CliError::input(format!(
                "{} does not accept all",
                field.flag()
            )));
        }
        return Ok(json!(amount));
    }

    match field.kind.as_str() {
        "boolean" => match python_strip(raw).to_ascii_lowercase().as_str() {
            "y" | "yes" | "true" | "1" => Ok(json!(true)),
            "n" | "no" | "false" | "0" => Ok(json!(false)),
            _ => Err(CliError::input(format!("expected y or n, got {raw:?}"))),
        },
        "integer" => integer(raw),
        "number" => number(raw),
        "native_number" => integer(raw).or_else(|_| number(raw)),
        "json" => rtao::primitives::json::from_str(raw)
            .map_err(|error| CliError::input(format!("invalid JSON: {error}"))),
        "object" => {
            let value: Value = rtao::primitives::json::from_str(raw)
                .map_err(|e| CliError::input(format!("invalid JSON: {e}")))?;
            if value.is_object() {
                Ok(value)
            } else {
                Err(CliError::input("expected a JSON object"))
            }
        }
        "array" => {
            let text = python_strip(raw);
            if !field.query && text.starts_with('[') {
                return rtao::primitives::json::from_str(text)
                    .map_err(|e| CliError::input(format!("invalid JSON list: {e}")));
            }
            let parts = text
                .split(',')
                .map(python_strip)
                .filter(|part| field.query || !part.is_empty());
            parts
                .map(|part| match annotation {
                    "list[int]" => integer(part),
                    "list[float]" => number(part),
                    _ => Ok(json!(part)),
                })
                .collect::<Result<Vec<_>, _>>()
                .map(Value::Array)
        }
        _ => Ok(json!(raw)),
    }
}
