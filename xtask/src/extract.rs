//! Rust-only extraction of a bounded Python literal subset from vendored inputs.

use std::{cell::Cell, collections::BTreeMap, fs, path::Path};

use anyhow::{Context, Result, bail, ensure};
use rustpython_parser::{
    Parse,
    ast::{self, Ranged, bigint::BigInt},
};
use serde_json::{Value, json};
use sha1::{Digest, Sha1};
use toml_edit::DocumentMut;

/// Intents in the pinned upstream catalog; a change requires review of the
/// complete generated surface.
pub(crate) const INTENT_COUNT: usize = 86;
/// Reads in the pinned upstream catalog; a change requires review of the
/// complete generated surface.
pub(crate) const READ_COUNT: usize = 103;

#[path = "extract_resolved.rs"]
mod resolved;

pub(crate) fn public_exports(root: &Path) -> Result<BTreeMap<String, Vec<String>>> {
    resolved::public_exports(root)
}

pub(crate) fn check_at(root: &Path) -> Result<()> {
    extract_at(root, true)?;
    resolved::extract_at(root, true)?;
    crate::extract_receipt::extract_at(root, true)
}

#[path = "extract_effects.rs"]
mod effects;

const SOURCE: &str = "sdk/python/tests/unit/test_http_auth.py";
const OUTPUT: &str = "vendor/upstream/extracted/btauth.json";
const NAMES: &[&str] = &[
    "FIXED_NONCE",
    "BODY",
    "METHOD",
    "PATH",
    "GOLDEN_SR25519_PAYLOAD",
    "GOLDEN_ED25519_SIGNATURE",
];
// These bounds prevent huge powers, string multiplication and deep expressions
// from turning a source change into an unbounded extraction job.
const MAX_STRING_BYTES: usize = 1024 * 1024;
const MAX_DEPTH: usize = 128;
const MAX_ALLOCATION_UNITS: usize = 8 * 1024 * 1024;
const MAX_SOURCE_BYTES: usize = 8 * 1024 * 1024;
const MAX_CONTAINER_ITEMS: usize = 4096;

#[derive(Clone, Debug, PartialEq)]
enum Literal {
    None,
    Bool(bool),
    Int(BigInt),
    Float(f64),
    Str(String),
    Bytes(Vec<u8>),
    List(Vec<Self>),
    Tuple(Vec<Self>),
    Dict(Vec<(Self, Self)>),
    Set(Vec<Self>),
    FrozenSet(Vec<Self>),
    Resolved(Value),
}

impl Literal {
    fn allocation_units(&self, depth: usize) -> Result<usize> {
        ensure!(
            depth < MAX_DEPTH,
            "resolved literal exceeds depth {MAX_DEPTH}"
        );
        let mut units = 64usize;
        match self {
            Self::Str(value) => units = units.saturating_add(value.len()),
            Self::Bytes(value) => units = units.saturating_add(value.len()),
            Self::Resolved(value) => units = units.saturating_add(value.to_string().len()),
            Self::List(values)
            | Self::Tuple(values)
            | Self::Set(values)
            | Self::FrozenSet(values) => {
                ensure!(
                    values.len() <= MAX_CONTAINER_ITEMS,
                    "literal container exceeds item limit"
                );
                for value in values {
                    units = units.saturating_add(value.allocation_units(depth + 1)?);
                }
            }
            Self::Dict(entries) => {
                ensure!(
                    entries.len() <= MAX_CONTAINER_ITEMS,
                    "literal dictionary exceeds item limit"
                );
                for (key, value) in entries {
                    units = units
                        .saturating_add(key.allocation_units(depth + 1)?)
                        .saturating_add(value.allocation_units(depth + 1)?);
                }
            }
            _ => {}
        }
        ensure!(
            units <= MAX_ALLOCATION_UNITS,
            "resolved literal exceeds allocation limit"
        );
        Ok(units)
    }

    fn json(&self) -> Result<Value> {
        Ok(match self {
            Self::None => Value::Null,
            Self::Bool(value) => json!(value),
            // Parsing the decimal token preserves integers beyond u64. Never
            // route Python integers through f64 or a fixed-width Rust integer.
            Self::Int(value) => serde_json::from_str(&value.to_string())?,
            Self::Float(value) => json!(value),
            Self::Str(value) => json!(value),
            Self::Bytes(value) => json!({"bytes_hex": hex(value)}),
            Self::List(values) => Value::Array(json_values(values)?),
            Self::Tuple(values) => json!({"tuple": json_values(values)?}),
            Self::Dict(entries) => {
                let entries = entries
                    .iter()
                    .map(|(key, value)| Ok(json!([key.json()?, value.json()?])))
                    .collect::<Result<Vec<_>>>()?;
                json!({"dict": entries})
            }
            Self::Set(values) => json!({"set": sorted_json(values)?}),
            Self::FrozenSet(values) => json!({"frozenset": sorted_json(values)?}),
            Self::Resolved(value) => value.clone(),
        })
    }

    fn hashable(&self) -> bool {
        match self {
            Self::List(_) | Self::Dict(_) | Self::Set(_) | Self::Resolved(_) => false,
            Self::Tuple(values) | Self::FrozenSet(values) => values.iter().all(Self::hashable),
            _ => true,
        }
    }
}

fn json_values(values: &[Literal]) -> Result<Vec<Value>> {
    values.iter().map(Literal::json).collect()
}

fn sorted_json(values: &[Literal]) -> Result<Vec<Value>> {
    let mut values = json_values(values)?;
    values.sort_by_cached_key(Value::to_string);
    Ok(values)
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        // String writes cannot fail.
        let _ = write!(value, "{byte:02x}");
    }
    value
}

#[derive(Clone)]
enum Binding {
    Value(Literal),
    Refused(String),
}

type Bindings = BTreeMap<String, Binding>;

struct Evaluator<'a> {
    bindings: &'a Bindings,
    budget: &'a Cell<usize>,
}

impl Evaluator<'_> {
    fn eval(&self, expression: &ast::Expr, depth: usize) -> Result<Literal> {
        ensure!(
            depth < MAX_DEPTH,
            "literal expression exceeds depth {MAX_DEPTH}"
        );
        spend(self.budget, 64)?;
        let next = depth + 1;
        match expression {
            ast::Expr::Constant(expression) => {
                match &expression.value {
                    ast::Constant::Str(value) => spend(self.budget, value.len())?,
                    ast::Constant::Bytes(value) => spend(self.budget, value.len())?,
                    _ => {}
                }
                constant(&expression.value)
            }
            ast::Expr::Name(expression) => match self.bindings.get(expression.id.as_str()) {
                Some(Binding::Value(value)) => {
                    spend(self.budget, value.allocation_units(depth)?)?;
                    Ok(value.clone())
                }
                Some(Binding::Refused(reason)) => {
                    bail!(
                        "name {} is not an accepted literal: {reason}",
                        expression.id
                    )
                }
                None => bail!("unresolved name {}", expression.id),
            },
            ast::Expr::List(expression) => {
                Ok(Literal::List(self.sequence(&expression.elts, next)?))
            }
            ast::Expr::Tuple(expression) => {
                Ok(Literal::Tuple(self.sequence(&expression.elts, next)?))
            }
            ast::Expr::Set(expression) => Ok(Literal::Set(unique_hashable(
                self.sequence(&expression.elts, next)?,
            )?)),
            ast::Expr::Dict(expression) => {
                let mut entries = Vec::new();
                for (key, value) in expression.keys.iter().zip(&expression.values) {
                    let key = key
                        .as_ref()
                        .context("dictionary unpacking is not supported")?;
                    let key = self.eval(key, next)?;
                    let value = self.eval(value, next)?;
                    ensure!(key.hashable(), "dictionary key is not hashable");
                    insert_entry(&mut entries, key, value);
                }
                Ok(Literal::Dict(entries))
            }
            ast::Expr::UnaryOp(expression) => {
                match (expression.op, self.eval(&expression.operand, next)?) {
                    (ast::UnaryOp::UAdd, Literal::Int(value)) => checked_int(value),
                    (ast::UnaryOp::USub, Literal::Int(value)) => checked_int(-value),
                    (ast::UnaryOp::UAdd, Literal::Float(value)) => checked_float(value),
                    (ast::UnaryOp::USub, Literal::Float(value)) => checked_float(-value),
                    _ => bail!("only unary +/- on numeric literals is supported"),
                }
            }
            ast::Expr::BinOp(expression) => binary(
                expression.op,
                self.eval(&expression.left, next)?,
                self.eval(&expression.right, next)?,
                self.budget,
            ),
            ast::Expr::Call(expression) => self.constructor(expression, next),
            _ => bail!("unsupported Python expression; extraction never executes source code"),
        }
    }

    fn sequence(&self, expressions: &[ast::Expr], depth: usize) -> Result<Vec<Literal>> {
        ensure!(
            expressions.len() <= MAX_CONTAINER_ITEMS,
            "literal container exceeds item limit"
        );
        expressions
            .iter()
            .map(|expression| self.eval(expression, depth))
            .collect()
    }

    fn constructor(&self, call: &ast::ExprCall, depth: usize) -> Result<Literal> {
        let ast::Expr::Name(function) = &*call.func else {
            bail!("only explicitly whitelisted builtin constructors are supported");
        };
        let name = function.id.as_str();
        ensure!(
            matches!(
                name,
                "list" | "tuple" | "dict" | "set" | "frozenset" | "bytes"
            ),
            "constructor {name:?} is not whitelisted"
        );
        ensure!(
            !self.bindings.contains_key(name),
            "builtin constructor {name} is shadowed"
        );
        ensure!(
            call.args.len() <= 1,
            "constructor {name} accepts at most one positional argument"
        );
        if name == "dict" {
            let mut entries = if let Some(argument) = call.args.first() {
                match self.eval(argument, depth)? {
                    Literal::Dict(entries) => entries,
                    Literal::List(items) | Literal::Tuple(items) => {
                        let mut entries = Vec::new();
                        for item in items {
                            let pair = match item {
                                Literal::List(values) | Literal::Tuple(values)
                                    if values.len() == 2 =>
                                {
                                    values
                                }
                                _ => bail!("dict input must contain literal key/value pairs"),
                            };
                            let mut pair = pair.into_iter();
                            let key = pair.next().context("missing dict key")?;
                            let value = pair.next().context("missing dict value")?;
                            ensure!(key.hashable(), "dictionary key is not hashable");
                            insert_entry(&mut entries, key, value);
                        }
                        entries
                    }
                    _ => bail!("dict input must be a literal dictionary or key/value sequence"),
                }
            } else {
                Vec::new()
            };
            for keyword in &call.keywords {
                let key = keyword
                    .arg
                    .as_ref()
                    .context("keyword unpacking is not supported")?;
                insert_entry(
                    &mut entries,
                    Literal::Str(key.to_string()),
                    self.eval(&keyword.value, depth)?,
                );
            }
            return Ok(Literal::Dict(entries));
        }
        ensure!(
            call.keywords.is_empty(),
            "constructor {name} does not accept keyword arguments"
        );
        let (values, unordered) = match call.args.first() {
            Some(expression) => match self.eval(expression, depth)? {
                Literal::List(values) | Literal::Tuple(values) => (values, false),
                Literal::Set(values) | Literal::FrozenSet(values) => (values, true),
                // Iteration order of a set is deliberately unavailable. Sets may
                // be converted only to another set, whose JSON order is canonical.
                _ => bail!("constructor {name} needs a literal container"),
            },
            None => (Vec::new(), false),
        };
        ensure!(
            !(unordered && matches!(name, "list" | "tuple" | "bytes")),
            "set iteration order cannot be extracted into {name}"
        );
        match name {
            "list" => Ok(Literal::List(values)),
            "tuple" => Ok(Literal::Tuple(values)),
            "set" => Ok(Literal::Set(unique_hashable(values)?)),
            "frozenset" => Ok(Literal::FrozenSet(unique_hashable(values)?)),
            "bytes" => {
                let bytes = values
                    .into_iter()
                    .map(|value| match value {
                        Literal::Int(value) => value
                            .to_string()
                            .parse::<u8>()
                            .context("bytes element must be in 0..=255"),
                        _ => bail!("bytes elements must be integer literals"),
                    })
                    .collect::<Result<Vec<_>>>()?;
                Ok(Literal::Bytes(bytes))
            }
            _ => bail!("unsupported constructor {name}"),
        }
    }
}

fn constant(value: &ast::Constant) -> Result<Literal> {
    match value {
        ast::Constant::None => Ok(Literal::None),
        ast::Constant::Bool(value) => Ok(Literal::Bool(*value)),
        ast::Constant::Str(value) => {
            ensure!(
                value.len() <= MAX_STRING_BYTES,
                "string literal exceeds byte limit"
            );
            Ok(Literal::Str(value.clone()))
        }
        ast::Constant::Bytes(value) => {
            ensure!(
                value.len() <= MAX_STRING_BYTES,
                "bytes literal exceeds byte limit"
            );
            Ok(Literal::Bytes(value.clone()))
        }
        ast::Constant::Int(value) => checked_int(value.clone()),
        ast::Constant::Float(value) => checked_float(*value),
        ast::Constant::Tuple(values) => Ok(Literal::Tuple(
            values.iter().map(constant).collect::<Result<_>>()?,
        )),
        _ => bail!("complex numbers and ellipsis are outside the literal subset"),
    }
}

fn spend(budget: &Cell<usize>, units: usize) -> Result<()> {
    let remaining = budget
        .get()
        .checked_sub(units)
        .context("literal extraction exceeds aggregate allocation limit")?;
    budget.set(remaining);
    Ok(())
}

fn checked_int(value: BigInt) -> Result<Literal> {
    ensure!(
        value.bits() <= 256,
        "integer literal/fold exceeds |value| < 2^256"
    );
    Ok(Literal::Int(value))
}

fn checked_float(value: f64) -> Result<Literal> {
    ensure!(
        value.is_finite(),
        "non-finite floats are outside the JSON literal subset"
    );
    Ok(Literal::Float(value))
}

fn binary(
    op: ast::Operator,
    left: Literal,
    right: Literal,
    budget: &Cell<usize>,
) -> Result<Literal> {
    match (left, right) {
        (Literal::Int(left), Literal::Int(right)) => integer_binary(op, left, right),
        (Literal::Float(left), Literal::Float(right)) => float_binary(op, left, right),
        (Literal::Float(left), Literal::Int(right)) => {
            float_binary(op, left, right.to_string().parse::<f64>()?)
        }
        (Literal::Int(left), Literal::Float(right)) => {
            float_binary(op, left.to_string().parse::<f64>()?, right)
        }
        (Literal::Str(mut left), Literal::Str(right)) if op == ast::Operator::Add => {
            ensure!(
                left.len().saturating_add(right.len()) <= MAX_STRING_BYTES,
                "folded string exceeds byte limit"
            );
            spend(budget, left.len().saturating_add(right.len()))?;
            left.push_str(&right);
            Ok(Literal::Str(left))
        }
        (Literal::Str(value), Literal::Int(count)) | (Literal::Int(count), Literal::Str(value))
            if op == ast::Operator::Mult =>
        {
            if count < BigInt::from(0) || value.is_empty() {
                return Ok(Literal::Str(String::new()));
            }
            let count = count
                .to_string()
                .parse::<usize>()
                .context("string repeat count exceeds limit")?;
            ensure!(
                value
                    .len()
                    .checked_mul(count)
                    .is_some_and(|size| size <= MAX_STRING_BYTES),
                "folded string exceeds byte limit"
            );
            spend(budget, value.len() * count)?;
            Ok(Literal::Str(value.repeat(count)))
        }
        _ => bail!("operator {op:?} is not supported for these literal types"),
    }
}

fn integer_binary(op: ast::Operator, left: BigInt, right: BigInt) -> Result<Literal> {
    let zero = BigInt::from(0);
    let value = match op {
        ast::Operator::Add => left + right,
        ast::Operator::Sub => left - right,
        ast::Operator::Mult => left * right,
        ast::Operator::FloorDiv | ast::Operator::Mod => {
            ensure!(right != zero, "integer division by zero");
            let mut quotient = &left / &right;
            let mut remainder = &left % &right;
            if remainder != zero && (left < zero) != (right < zero) {
                quotient -= 1;
                remainder += &right;
            }
            if op == ast::Operator::FloorDiv {
                quotient
            } else {
                remainder
            }
        }
        ast::Operator::Pow => {
            ensure!(
                right >= zero,
                "negative integer powers produce floats and are not supported"
            );
            if right == zero {
                BigInt::from(1)
            } else if left == zero {
                zero
            } else if left == BigInt::from(1) {
                left
            } else if left == BigInt::from(-1) {
                if (right & BigInt::from(1)) == zero {
                    BigInt::from(1)
                } else {
                    BigInt::from(-1)
                }
            } else {
                let exponent = right
                    .to_string()
                    .parse::<u32>()
                    .context("integer power exceeds 256-bit limit")?;
                ensure!(exponent <= 256, "integer power exceeds 256-bit limit");
                left.pow(exponent)
            }
        }
        ast::Operator::LShift | ast::Operator::RShift => {
            ensure!(right >= zero, "negative shift count");
            if right > BigInt::from(256) {
                if op == ast::Operator::RShift {
                    if left < zero { BigInt::from(-1) } else { zero }
                } else {
                    ensure!(left == zero, "left shift exceeds 256-bit limit");
                    zero
                }
            } else {
                let shift = right.to_string().parse::<usize>()?;
                if op == ast::Operator::LShift {
                    left << shift
                } else {
                    left >> shift
                }
            }
        }
        ast::Operator::BitOr => left | right,
        ast::Operator::BitAnd => left & right,
        _ => bail!("integer operator {op:?} is outside the literal subset"),
    };
    checked_int(value)
}

fn float_binary(op: ast::Operator, left: f64, right: f64) -> Result<Literal> {
    let value = match op {
        ast::Operator::Add => left + right,
        ast::Operator::Sub => left - right,
        ast::Operator::Mult => left * right,
        ast::Operator::Div => {
            ensure!(right != 0.0, "float division by zero");
            left / right
        }
        _ => bail!("float operator {op:?} is outside the literal subset"),
    };
    checked_float(value)
}

// Python aliases bools and numerically equal int/float keys in dicts and sets.
fn equal_key(left: &Literal, right: &Literal) -> bool {
    match (left, right) {
        (Literal::Bool(left), Literal::Int(right)) | (Literal::Int(right), Literal::Bool(left)) => {
            right == &BigInt::from(u8::from(*left))
        }
        (Literal::Float(left), Literal::Int(right))
        | (Literal::Int(right), Literal::Float(left)) => {
            // Decimal formatting of an integral float recovers its exact
            // represented integer, without rounding the bigint through f64.
            if *left == 0.0 {
                right == &BigInt::from(0)
            } else {
                left.fract() == 0.0 && format!("{left:.0}") == right.to_string()
            }
        }
        (Literal::Float(left), Literal::Bool(right))
        | (Literal::Bool(right), Literal::Float(left)) => *left == f64::from(u8::from(*right)),
        (Literal::Tuple(left), Literal::Tuple(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right)
                    .all(|(left, right)| equal_key(left, right))
        }
        (Literal::FrozenSet(left), Literal::FrozenSet(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .all(|left| right.iter().any(|right| equal_key(left, right)))
        }
        _ => left == right,
    }
}

fn unique_hashable(values: Vec<Literal>) -> Result<Vec<Literal>> {
    let mut unique = Vec::new();
    for value in values {
        ensure!(value.hashable(), "set element is not hashable");
        if !unique.iter().any(|item| equal_key(item, &value)) {
            unique.push(value);
        }
    }
    Ok(unique)
}

fn insert_entry(entries: &mut Vec<(Literal, Literal)>, key: Literal, value: Literal) {
    if let Some((_, old_value)) = entries
        .iter_mut()
        .find(|(old_key, _)| equal_key(old_key, &key))
    {
        *old_value = value;
    } else {
        entries.push((key, value));
    }
}

fn line(source: &str, offset: usize) -> usize {
    source.as_bytes()[..offset]
        .iter()
        .filter(|byte| **byte == b'\n')
        .count()
        + 1
}

fn check_function_defaults(
    args: &ast::Arguments,
    bindings: &Bindings,
    budget: &Cell<usize>,
    source: &str,
    path: &str,
) -> Result<()> {
    for argument in args
        .posonlyargs
        .iter()
        .chain(&args.args)
        .chain(&args.kwonlyargs)
    {
        if let Some(default) = &argument.default {
            Evaluator { bindings, budget }
                .eval(default, 0)
                .with_context(|| {
                    format!(
                        "{path}:{}: function default is outside the literal subset",
                        line(source, default.range().start().to_usize())
                    )
                })?;
        }
    }
    Ok(())
}

fn check_decorators(decorators: &[ast::Expr], source: &str, path: &str) -> Result<()> {
    for decorator in decorators {
        let text =
            &source[decorator.range().start().to_usize()..decorator.range().end().to_usize()];
        for token in rustpython_parser::lexer::lex(text, rustpython_parser::Mode::Expression) {
            let (token, _) = token.map_err(|error| anyhow::anyhow!("{path}: {error:?}"))?;
            ensure!(
                !matches!(token, rustpython_parser::Tok::ColonEqual),
                "{path}:{}: assignment expression in a decorator is outside the literal subset",
                line(source, decorator.range().start().to_usize())
            );
        }
    }
    Ok(())
}

fn extract_values(source: &str, path: &str, names: &[&str]) -> Result<Value> {
    ensure!(
        source.len() <= MAX_SOURCE_BYTES,
        "{path}: Python source exceeds extraction byte limit"
    );
    let suite =
        ast::Suite::parse(source, path).map_err(|error| anyhow::anyhow!("{path}: {error}"))?;
    let mut bindings = Bindings::new();
    let mut locations = BTreeMap::new();
    let budget = Cell::new(MAX_ALLOCATION_UNITS);
    for statement in &suite {
        let (targets, value) = match statement {
            ast::Stmt::Assign(assignment) => (&assignment.targets[..], &*assignment.value),
            ast::Stmt::AnnAssign(assignment) => {
                let Some(value) = &assignment.value else {
                    continue;
                };
                (std::slice::from_ref(&*assignment.target), &**value)
            }
            ast::Stmt::FunctionDef(function) => {
                check_function_defaults(&function.args, &bindings, &budget, source, path)?;
                check_decorators(&function.decorator_list, source, path)?;
                bindings.insert(
                    function.name.to_string(),
                    Binding::Refused("function definition".into()),
                );
                continue;
            }
            ast::Stmt::AsyncFunctionDef(function) => {
                check_function_defaults(&function.args, &bindings, &budget, source, path)?;
                check_decorators(&function.decorator_list, source, path)?;
                bindings.insert(
                    function.name.to_string(),
                    Binding::Refused("function definition".into()),
                );
                continue;
            }
            ast::Stmt::ClassDef(class) => {
                bail!(
                    "{path}:{}: class definition executes a body and is outside the v0 literal subset",
                    line(source, class.range.start().to_usize())
                );
            }
            ast::Stmt::Import(import) => {
                for alias in &import.names {
                    let name = alias.asname.as_ref().map_or_else(
                        || alias.name.split('.').next().unwrap_or(""),
                        |name| name.as_str(),
                    );
                    bindings.insert(name.to_string(), Binding::Refused("imported module".into()));
                }
                continue;
            }
            ast::Stmt::ImportFrom(import) => {
                for alias in &import.names {
                    let name = alias.asname.as_ref().unwrap_or(&alias.name);
                    ensure!(
                        name.as_str() != "*",
                        "{path}:{}: wildcard imports prevent safe literal name resolution",
                        line(source, statement.range().start().to_usize())
                    );
                    bindings.insert(
                        name.to_string(),
                        Binding::Refused(
                            "import resolution belongs to the extended extractor".into(),
                        ),
                    );
                }
                continue;
            }
            ast::Stmt::AugAssign(assignment) => {
                bail!(
                    "{path}:{}: augmented assignment can mutate aliased literals and is outside the subset",
                    line(source, assignment.range.start().to_usize())
                );
            }
            ast::Stmt::Delete(delete) => {
                bail!(
                    "{path}:{}: deletion can mutate aliased literals and is outside the subset",
                    line(source, delete.range.start().to_usize())
                );
            }
            ast::Stmt::If(_)
            | ast::Stmt::For(_)
            | ast::Stmt::AsyncFor(_)
            | ast::Stmt::While(_)
            | ast::Stmt::With(_)
            | ast::Stmt::AsyncWith(_)
            | ast::Stmt::Try(_)
            | ast::Stmt::TryStar(_)
            | ast::Stmt::Match(_) => {
                // A control-flow block can conditionally rebind any module name.
                // Refuse rather than silently reporting a preceding assignment.
                bail!(
                    "{path}:{}: module-level control flow is outside the literal subset",
                    line(source, statement.range().start().to_usize())
                );
            }
            ast::Stmt::Expr(expression) => {
                ensure!(
                    matches!(&*expression.value, ast::Expr::Constant(constant) if matches!(&constant.value, ast::Constant::Str(_))),
                    "{path}:{}: module-level expression can mutate literals and is outside the subset",
                    line(source, statement.range().start().to_usize())
                );
                continue;
            }
            _ => continue,
        };
        let evaluated = Evaluator {
            bindings: &bindings,
            budget: &budget,
        }
        .eval(value, 0)
        .with_context(|| {
            format!(
                "{path}:{}: module assignment is outside the literal subset",
                line(source, statement.range().start().to_usize())
            )
        })?;
        for target in targets {
            let ast::Expr::Name(name) = target else {
                bail!(
                    "{path}:{}: unpacking or mutation assignment is outside the literal subset",
                    line(source, statement.range().start().to_usize())
                );
            };
            spend(&budget, evaluated.allocation_units(0)?)?;
            let binding = Binding::Value(evaluated.clone());
            bindings.insert(name.id.to_string(), binding);
            locations.insert(
                name.id.to_string(),
                (
                    line(source, statement.range().start().to_usize()),
                    line(source, statement.range().end().to_usize()),
                ),
            );
        }
    }
    let mut values = serde_json::Map::new();
    for name in names {
        let value = match bindings.get(*name) {
            Some(Binding::Value(value)) => value,
            Some(Binding::Refused(reason)) => bail!("cannot extract {name}: {reason}"),
            None => bail!("{path}: required literal {name} has no module-level assignment"),
        };
        value.allocation_units(0)?;
        let (start, end) = locations
            .get(*name)
            .context("literal has no source location")?;
        values.insert(
            (*name).into(),
            json!({"line":start,"end_line":end,"value":value.json()?}),
        );
    }
    Ok(Value::Object(values))
}

pub(crate) fn git_blob(bytes: &[u8]) -> String {
    let mut hash = Sha1::new();
    hash.update(format!("blob {}\0", bytes.len()).as_bytes());
    hash.update(bytes);
    format!("{:x}", hash.finalize())
}

fn render(root: &Path) -> Result<Vec<u8>> {
    let pins: DocumentMut = fs::read_to_string(root.join("upstream.toml"))?.parse()?;
    let tag = pins["parity"]["tag"]
        .as_str()
        .context("upstream.toml missing parity.tag")?;
    let commit = pins["parity"]["commit"]
        .as_str()
        .context("upstream.toml missing parity.commit")?;
    let bytes = fs::read(root.join("vendor/upstream/src").join(SOURCE))
        .with_context(|| format!("read vendored extraction input {SOURCE}"))?;
    let blob = git_blob(&bytes);
    let tree = fs::read_to_string(root.join("vendor/upstream/tree.txt"))?;
    let entry = tree
        .lines()
        .find(|entry| {
            entry
                .split_once('\t')
                .is_some_and(|(_, path)| path == SOURCE)
        })
        .context("btauth input missing from watched upstream tree")?;
    let metadata = entry.split_once('\t').context("malformed tree entry")?.0;
    let expected = metadata
        .split_whitespace()
        .nth(2)
        .context("missing git blob SHA in tree entry")?;
    ensure!(
        blob == expected,
        "vendored input {SOURCE} is modified: expected blob {expected}, found {blob}"
    );
    let source = std::str::from_utf8(&bytes).context("vendored Python source is not UTF-8")?;
    let mut output = json!({
        "format": 1,
        "provenance": {"tag":tag,"commit":commit,"path":SOURCE,"blob":blob},
        "values": extract_values(source, SOURCE, NAMES)?,
    });
    // Extraction must be byte-stable if unrelated workspace features enable
    // serde_json/preserve_order. Python ordered dictionaries use entry arrays.
    output.sort_all_objects();
    let mut bytes = serde_json::to_vec_pretty(&output)?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn extract_at(root: &Path, check: bool) -> Result<()> {
    let expected = render(root)?;
    let path = root.join(OUTPUT);
    if check {
        let actual =
            fs::read(&path).with_context(|| format!("read {OUTPUT}; run cargo xtask extract"))?;
        ensure!(
            actual == expected,
            "{OUTPUT} is stale or modified; run cargo xtask extract"
        );
    } else if fs::read(&path).ok().as_deref() != Some(expected.as_slice()) {
        fs::create_dir_all(path.parent().context("output path has no parent")?)?;
        fs::write(path, expected)?;
    }
    Ok(())
}

/// Extract btauth goldens and Python tables, or verify their committed outputs.
pub fn run(args: &[String]) -> Result<()> {
    let mut check = false;
    for arg in args {
        match arg.as_str() {
            "--check" => check = true,
            "--help" | "-h" => {
                println!(
                    "cargo xtask extract [--check]\n\nRust-only literal extraction from vendored upstream Python. --check is read-only and offline."
                );
                return Ok(());
            }
            _ => bail!("unknown extract argument {arg:?}"),
        }
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .context("xtask has no workspace parent")?;
    extract_at(root, check)?;
    resolved::extract_at(root, check)?;
    crate::extract_receipt::extract_at(root, check)?;
    println!(
        "Python table extraction {} (btauth, settings, RPC, named tables)",
        if check { "verified" } else { "refreshed" }
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value(source: &str) -> Result<Value> {
        Ok(extract_values(source, "literal_test.py", &["VALUE"])?["VALUE"]["value"].clone())
    }

    #[test]
    fn accepts_literals_and_preserves_exact_integer_and_bytes() -> Result<()> {
        assert_eq!(
            value("VALUE = 1752076800000000000")?.to_string(),
            "1752076800000000000"
        );
        assert_eq!(
            value("VALUE = 2**128 + 1")?.to_string(),
            "340282366920938463463374607431768211457"
        );
        assert_eq!(
            value("VALUE = b'\\x00\\xff' b'hello'")?,
            json!({"bytes_hex":"00ff68656c6c6f"})
        );
        assert_eq!(
            value("VALUE = ('hello' ' world', None, True, -1.25)")?,
            json!({"tuple":["hello world",null,true,-1.25]})
        );
        assert_eq!(value("VALUE = [1, 2, +3]")?, json!([1, 2, 3]));
        assert_eq!(
            value("VALUE = {'a': 1, 'a': 2, 3: b'x'}")?,
            json!({"dict":[["a",2],[3,{"bytes_hex":"78"}]]})
        );
        Ok(())
    }

    #[test]
    fn folds_bounded_constants_with_python_division_semantics() -> Result<()> {
        assert_eq!(
            value("VALUE = [(-7)//3, (-7)%3, 7//(-3), 7%(-3)]")?,
            json!([-3, 2, -3, -2])
        );
        assert_eq!(
            value("VALUE = ['x' * 3 + 'y', 2 * 'ab', 'x' * -1]")?,
            json!(["xxxy", "abab", ""])
        );
        assert_eq!(
            value("VALUE = [1 << 80, (-4) >> 1000, (7 | 8) & 3, 1.5 / 2]")?,
            serde_json::from_str::<Value>("[1208925819614629174706176,-1,3,0.75]")?
        );
        assert_eq!(
            value("VALUE = 2**255")?.to_string(),
            "57896044618658097711785492504343953926634992332820282019728792003956564819968"
        );
        Ok(())
    }

    #[test]
    fn resolves_prior_module_names_and_whitelisted_constructors() -> Result<()> {
        assert_eq!(
            value("BASE = 10**9\nVALUE = frozenset({BASE, -32004, -32005})")?,
            json!({"frozenset":[-32004,-32005,1000000000]})
        );
        assert_eq!(value("BASE = 5\nVALUE = BASE\nBASE = 7")?, json!(5));
        assert_eq!(
            value("VALUE = dict([('a', 1)], b=tuple([2]), c=bytes([0,255]))")?,
            json!({"dict":[["a",1],["b",{"tuple":[2]}],["c",{"bytes_hex":"00ff"}]]})
        );
        assert_eq!(
            value("VALUE = [set(), frozenset(), dict(), tuple(), list(), bytes()]")?,
            json!([{"set":[]},{"frozenset":[]},{"dict":[]},{"tuple":[]},[],{"bytes_hex":""}])
        );
        Ok(())
    }

    #[test]
    fn preserves_python_set_and_dict_numeric_key_equality() -> Result<()> {
        assert_eq!(
            value("VALUE = {True, 1, 1.0, 9007199254740993, 9007199254740992.0}")?,
            json!({"set":[9007199254740992.0,9007199254740993_i64,true]})
        );
        assert_eq!(
            value("VALUE = {True: 'first', 1: 'second', 1.0: 'last'}")?,
            json!({"dict":[[true,"last"]]})
        );
        assert_eq!(
            value("VALUE = {-0.0: 1, 0: 2}")?,
            json!({"dict":[[-0.0,2]]})
        );
        Ok(())
    }

    #[test]
    fn refuses_code_execution_and_reports_source_line() {
        for expression in [
            "__import__('os').system('touch BAD')",
            "open('/tmp/BAD', 'w')",
            "[x for x in [1]]",
            "lambda: 1",
            "f'{1}'",
            "{**{'a': 1}}",
            "list({1, 2})",
            "unknown",
            "bytes([256])",
            "2**256",
            "'x' * 100000000000",
            "1e309",
            "1 // 0",
            "1.0 / 0",
            "{[1]}",
        ] {
            let source = format!("# source\nVALUE = {expression}\n");
            let error = value(&source).err().map(|error| format!("{error:#}"));
            assert!(
                error
                    .as_ref()
                    .is_some_and(|error| error.contains("literal_test.py:2:")),
                "{expression}: {error:?}"
            );
        }
        assert!(value("frozenset = 1\nVALUE = frozenset([2])").is_err());
        assert!(value("from somewhere import list\nVALUE = list([2])").is_err());
        assert!(value("VALUE = 1\nVALUE += 1").is_err());
        assert!(value("VALUE = 1\ndel VALUE").is_err());
        assert!(value("VALUE = 1\nVALUE, OTHER = (2, 3)").is_err());
        assert!(value("VALUE = [1]\nVALUE[0] = 2").is_err());
        assert!(value("VALUE = 1\nif True:\n    VALUE = 2").is_err());
        assert!(value("from somewhere import *\nVALUE = frozenset([1])").is_err());
        assert!(value("VALUE = [1]\nVALUE.append(2)").is_err());
        assert!(value("VALUE = [1]\nALIAS = VALUE\nALIAS[0] = 2").is_err());
        assert!(value("BASE = 1\nOTHER = (BASE := 2)\nVALUE = BASE").is_err());
        assert!(value("VALUE = [1,2]\nALIAS = VALUE\ndel ALIAS[0]").is_err());
        assert!(value("VALUE = [1]\nALIAS = VALUE\nALIAS[0], OTHER = (2,3)").is_err());
        assert!(value("VALUE = 1\ndef f(arg=(VALUE := 2)):\n    pass").is_err());
        assert!(value("VALUE = [1]\ndef f(arg=VALUE.append(2)):\n    pass").is_err());
        assert!(value("VALUE = 1\n@(VALUE := 2)\ndef f():\n    pass").is_err());
        assert!(value("VALUE = 1\nclass C:\n    global VALUE\n    VALUE = 2").is_err());
    }

    #[test]
    fn resolved_names_cannot_expand_past_resource_bounds() {
        let mut source = "VALUE = [0]\n".to_string();
        for _ in 0..30 {
            source.push_str("VALUE = [VALUE, VALUE]\n");
        }
        assert!(value(&source).is_err());
        let mut source = "VALUE = [0]\n".to_string();
        for _ in 0..MAX_DEPTH {
            source.push_str("VALUE = [VALUE]\n");
        }
        assert!(value(&source).is_err());
        let source = format!("VALUE = [{}]", vec!["0"; MAX_CONTAINER_ITEMS + 1].join(","));
        assert!(value(&source).is_err());
    }

    #[test]
    fn deterministic_render_and_check_detect_source_and_output_tampering() -> Result<()> {
        let root = std::env::var_os("RTAO_TEMP_ROOT")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .parent()
                    .unwrap_or(Path::new("."))
                    .join("target/extract-tests")
            });
        let root = root.join(format!("extract/check-{}", std::process::id()));
        let result = (|| -> Result<()> {
            fs::create_dir_all(root.join("vendor/upstream/src/sdk/python/tests/unit"))?;
            let source = b"FIXED_NONCE = 1752076800000000000\nBODY = b'body'\nMETHOD = 'POST'\nPATH = '/test'\nGOLDEN_SR25519_PAYLOAD = b'payload'\nGOLDEN_ED25519_SIGNATURE = '0x00'\n";
            fs::write(root.join("vendor/upstream/src").join(SOURCE), source)?;
            fs::write(
                root.join("vendor/upstream/tree.txt"),
                format!("100644 blob {}\t{SOURCE}\n", git_blob(source)),
            )?;
            fs::write(
                root.join("upstream.toml"),
                "[parity]\ntag = 'v478'\ncommit = '850f2755bb05a3851f948489f0ab27b0645cebce'\n",
            )?;
            assert!(extract_at(&root, true).is_err());
            extract_at(&root, false)?;
            let first = fs::read(root.join(OUTPUT))?;
            extract_at(&root, false)?;
            assert_eq!(first, fs::read(root.join(OUTPUT))?);
            extract_at(&root, true)?;
            let document: Value = serde_json::from_slice(&first)?;
            assert_eq!(document["values"]["FIXED_NONCE"]["line"], 1);
            assert_eq!(document["provenance"]["blob"], git_blob(source));
            fs::write(root.join(OUTPUT), b"{}\n")?;
            assert!(extract_at(&root, true).is_err());
            extract_at(&root, false)?;
            fs::write(
                root.join("vendor/upstream/src").join(SOURCE),
                b"BODY = b'tampered'\n",
            )?;
            assert!(render(&root).is_err());
            Ok(())
        })();
        if root.exists() {
            fs::remove_dir_all(&root)?;
        }
        result
    }
}
