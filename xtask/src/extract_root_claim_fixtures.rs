//! Test-only root-claim inputs projected from authenticated Python ASTs.
//!
//! This recognizes a finite set of source declarations and fixture shapes. It
//! does not evaluate decorators, exception constructors, or comprehensions.

use std::{
    collections::{BTreeMap, BTreeSet},
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

use super::super::effects;
use super::{Binding, Bindings, MAX_CONTAINER_ITEMS, Resolver, call_path, line, pretty};

const TEST: &str = "sdk/python/tests/unit/test_root_claim_fee.py";
const FEES: &str = "sdk/python/bittensor/intents/_root_claim_fee.py";
const OUTPUT: &str = "vendor/upstream/extracted/root_claim_fixtures.json";
const CONSTANTS: &[&str] = &[
    "_ROCKSDB_READ_PS",
    "_ROCKSDB_WRITE_PS",
    "_WEIGHT_FEE_PARTS",
    "_PERBILL",
    "_MAX_ROOT_CLAIM_WORK",
    "_MAX_ROOT_CLAIM_HOTKEY_WORK",
    "_MAX_BASKET_ROWS",
    "_FLUSH_BOUND_QUOTES",
    "_FLUSH_BOUND_ROWS",
    "_ROOT_CLAIM_FEE_ALLOWANCE",
    "_ROOT_CLAIM_CAP_459_REF_TIME",
    "_EXTENSION_FOLD_REF_TIME",
    "_DEFAULT_THRESHOLD_RAO",
];
const PARAMETRIZED: &[&str] = &[
    "test_discount_model_matches_the_467_release_node",
    "test_root_claim_too_heavy_is_a_hard_stop",
    "test_admission_hard_stop_survives_unavailable_payout_preview",
    "test_reserve_hard_stop_survives_unavailable_payout_preview",
    "test_admission_limit_is_inclusive_at_256",
];
const SEEDS: &[&str] = &[
    "test_coldkey_threshold_is_applied_per_hotkey_not_to_aggregate",
    "test_coldkey_quote_separates_eligible_and_below_threshold_hotkeys",
    "test_coldkey_quote_does_not_scan_validator_without_owed_shares",
    "test_shielded_root_claim_refuses_too_heavy_work_before_signing",
    "test_shielded_claim_reserves_free_tao_for_inner_and_carrier",
    "test_reserve_hard_stop_survives_unavailable_payout_preview",
    "test_carrier_hard_stop_survives_unavailable_payout_preview",
    "test_coldkey_wide_admission_ignores_non_root_hotkeys",
    "test_coldkey_wide_admission_keeps_unstaked_basket_claimant",
    "test_single_hotkey_quote_uses_the_dust_aware_preview_on_mixed_dust",
    "test_single_hotkey_quote_reports_an_all_dust_claim_as_a_noop",
    "test_coldkey_quote_applies_the_threshold_to_the_dust_aware_payout",
    "test_quote_falls_back_to_full_entitlement_without_the_preview_api",
    "test_quote_prices_consolidated_rows_as_full_claim_units",
    "test_quote_counts_consolidation_even_when_the_claim_is_below_threshold",
];
const PREVIEWS: &[&str] = &[
    "test_single_hotkey_quote_uses_the_dust_aware_preview_on_mixed_dust",
    "test_single_hotkey_quote_reports_an_all_dust_claim_as_a_noop",
    "test_coldkey_quote_applies_the_threshold_to_the_dust_aware_payout",
    "test_quote_prices_consolidated_rows_as_full_claim_units",
    "test_quote_counts_consolidation_even_when_the_claim_is_below_threshold",
];

fn name(expr: &ast::Expr, expected: &str) -> bool {
    matches!(expr, ast::Expr::Name(node) if node.id.as_str() == expected)
}

fn string(expr: &ast::Expr) -> Result<&str> {
    match expr {
        ast::Expr::Constant(node) => match &node.value {
            ast::Constant::Str(value) => Ok(value),
            _ => bail!("expected a literal string"),
        },
        _ => bail!("expected a literal string"),
    }
}

fn integer(expr: &ast::Expr) -> Result<u64> {
    let ast::Expr::Constant(node) = expr else {
        bail!("expected a literal integer")
    };
    let ast::Constant::Int(value) = &node.value else {
        bail!("expected a literal integer")
    };
    value
        .to_string()
        .parse::<u64>()
        .context("integer exceeds native u64")
}

fn count(expr: &ast::Expr) -> Result<u64> {
    let value = integer(expr)?;
    ensure!(value <= 1024, "fixture work/population exceeds bound");
    Ok(value)
}

fn tuple(expr: &ast::Expr, arity: usize) -> Result<&[ast::Expr]> {
    let ast::Expr::Tuple(node) = expr else {
        bail!("expected a tuple")
    };
    ensure!(node.elts.len() == arity, "tuple arity changed");
    Ok(&node.elts)
}

fn list(expr: &ast::Expr) -> Result<&[ast::Expr]> {
    let ast::Expr::List(node) = expr else {
        bail!("expected a list")
    };
    ensure!(
        node.elts.len() <= MAX_CONTAINER_ITEMS,
        "fixture list exceeds bound"
    );
    Ok(&node.elts)
}

fn call<'a>(expr: &'a ast::Expr, path: &str, arity: usize) -> Result<&'a ast::ExprCall> {
    let ast::Expr::Call(node) = expr else {
        bail!("expected {path} call")
    };
    ensure!(
        call_path(&node.func).as_deref() == Some(path)
            && node.args.len() == arity
            && node.keywords.is_empty(),
        "{path} call shape changed"
    );
    Ok(node)
}

struct Function<'a> {
    body: &'a [ast::Stmt],
    decorators: &'a [ast::Expr],
    args: &'a ast::Arguments,
}

fn function<'a>(suite: &'a [ast::Stmt], wanted: &str) -> Result<Function<'a>> {
    let mut found = None;
    for stmt in suite {
        let candidate = match stmt {
            ast::Stmt::FunctionDef(node) if node.name.as_str() == wanted => Some(Function {
                body: &node.body,
                decorators: &node.decorator_list,
                args: &node.args,
            }),
            ast::Stmt::AsyncFunctionDef(node) if node.name.as_str() == wanted => Some(Function {
                body: &node.body,
                decorators: &node.decorator_list,
                args: &node.args,
            }),
            _ => None,
        };
        if let Some(candidate) = candidate {
            ensure!(
                found.replace(candidate).is_none(),
                "duplicate function {wanted}"
            );
        }
    }
    found.with_context(|| format!("missing function {wanted}"))
}

fn assignment<'a>(suite: &'a [ast::Stmt], wanted: &str) -> Result<&'a ast::Expr> {
    let mut found = None;
    for stmt in suite {
        if let ast::Stmt::Assign(node) = stmt
            && node.targets.iter().any(|target| name(target, wanted))
        {
            ensure!(
                node.targets.len() == 1 && found.replace(node.value.as_ref()).is_none(),
                "duplicate/aliased assignment {wanted}"
            );
        }
    }
    found.with_context(|| format!("missing assignment {wanted}"))
}

fn span(path: &str, source: &str, expr: &ast::Expr) -> Value {
    json!({"path":path,"line":line(source,expr.range().start().to_usize()),"end_line":line(source,expr.range().end().to_usize()),
        "start_byte":expr.range().start().to_usize(),"end_byte":expr.range().end().to_usize()})
}

fn record(source: &str, symbol: &str, expr: &ast::Expr, value: Value) -> Value {
    let mut source = span(TEST, source, expr);
    source["symbol"] = json!(symbol);
    json!({"source":source,"value":value})
}

/// Reject executable expressions even where the shared resolver knows a call.
fn literal(resolver: &mut Resolver<'_>, bindings: &Bindings, expr: &ast::Expr) -> Result<Value> {
    let mut accepted = true;
    effects::walk(expr, 0, &mut |node| {
        accepted &= matches!(
            node,
            ast::Expr::Constant(_)
                | ast::Expr::Name(_)
                | ast::Expr::List(_)
                | ast::Expr::Tuple(_)
                | ast::Expr::Dict(_)
                | ast::Expr::UnaryOp(_)
                | ast::Expr::BinOp(_)
                | ast::Expr::Attribute(_)
        );
    })?;
    ensure!(accepted, "fixture expression is outside the literal subset");
    resolver.eval(expr, bindings, TEST, 0)?.json()
}

fn parameter<'a>(suite: &'a [ast::Stmt], wanted: &str, expected: &[&str]) -> Result<&'a ast::Expr> {
    let function = function(suite, wanted)?;
    let mut selected = None;
    for decorator in function.decorators {
        if call_path(decorator).as_deref() == Some("pytest.mark.asyncio") {
            continue;
        }
        let node = call(decorator, "pytest.mark.parametrize", 2)?;
        let actual = if expected.len() == 1 {
            vec![string(&node.args[0])?]
        } else {
            tuple(&node.args[0], expected.len())?
                .iter()
                .map(string)
                .collect::<Result<Vec<_>>>()?
        };
        ensure!(
            actual == expected && selected.replace(&node.args[1]).is_none(),
            "{wanted}: parameter names or multiplicity changed"
        );
    }
    selected.with_context(|| format!("{wanted}: missing parametrize"))
}

fn annotation(expr: &ast::Expr) -> bool {
    match expr {
        ast::Expr::Name(node) => matches!(
            node.id.as_str(),
            "str" | "int" | "bool" | "dict" | "list" | "FakeSubstrate"
        ),
        ast::Expr::Subscript(node) => {
            annotation(&node.value)
                && match &*node.slice {
                    ast::Expr::Tuple(tuple) => tuple.elts.iter().all(annotation),
                    expr => annotation(expr),
                }
        }
        ast::Expr::Constant(node) => node.value == ast::Constant::None,
        _ => false,
    }
}

/// Fixed-source module envelope. Unknown top-level code may mutate declarations.
fn envelope(suite: &[ast::Stmt]) -> Result<()> {
    let expected_imports = BTreeMap::from([
        ("__future__", vec![("annotations", "annotations")]),
        ("bittensor.balance", vec![("Balance", "Balance")]),
        ("bittensor.client", vec![("Client", "Client")]),
        ("bittensor.intents", vec![("_root_claim_fee", "fees")]),
        (
            "bittensor.intents.registration",
            vec![("ClaimRoot", "ClaimRoot")],
        ),
        ("bittensor.result", vec![("PolicyError", "PolicyError")]),
        (
            "tests.harness.fake_substrate",
            vec![("FakeSubstrate", "FakeSubstrate")],
        ),
        (
            "tests.harness.samples",
            vec![
                ("ALICE", "ALICE"),
                ("ALICE_HOT", "ALICE_HOT"),
                ("BOB", "BOB"),
                ("BOB_HOT", "BOB_HOT"),
                ("dev_wallet", "dev_wallet"),
            ],
        ),
    ]);
    let mut imports = BTreeSet::new();
    let mut declarations = BTreeSet::new();
    let mut functions = BTreeSet::new();
    let mut pytest = false;
    for (index, stmt) in suite.iter().enumerate() {
        match stmt {
            ast::Stmt::Expr(node) if index == 0 && string(&node.value).is_ok() => {}
            ast::Stmt::Import(node) => {
                ensure!(
                    !pytest
                        && node.names.len() == 1
                        && node.names[0].name.as_str() == "pytest"
                        && node.names[0].asname.is_none(),
                    "only unaliased pytest import is accepted"
                );
                pytest = true;
            }
            ast::Stmt::ImportFrom(node) => {
                let module = node
                    .module
                    .as_ref()
                    .context("relative fixture import")?
                    .as_str();
                ensure!(
                    node.level.is_none_or(|level| level.to_usize() == 0) && imports.insert(module),
                    "duplicate/relative fixture import"
                );
                let aliases = node
                    .names
                    .iter()
                    .map(|alias| {
                        (
                            alias.name.as_str(),
                            alias.asname.as_ref().unwrap_or(&alias.name).as_str(),
                        )
                    })
                    .collect::<Vec<_>>();
                ensure!(
                    expected_imports.get(module) == Some(&aliases),
                    "fixture import changed: {module}"
                );
            }
            ast::Stmt::Assign(node) => {
                ensure!(node.targets.len() == 1, "aliased fixture declaration");
                let ast::Expr::Name(target) = &node.targets[0] else {
                    bail!("fixture declaration target changed")
                };
                ensure!(
                    matches!(target.id.as_str(), "_EXTENSION_REF_TIME" | "_NODE_QUOTES")
                        && declarations.insert(target.id.as_str()),
                    "extra/rebound fixture declaration"
                );
            }
            ast::Stmt::FunctionDef(node) => {
                ensure!(
                    node.returns.as_ref().is_none_or(|expr| annotation(expr)),
                    "executable fixture return annotation"
                );
                ensure!(
                    functions.insert(node.name.as_str()),
                    "duplicate fixture function"
                );
            }
            ast::Stmt::AsyncFunctionDef(node) => {
                ensure!(
                    node.returns.as_ref().is_none_or(|expr| annotation(expr)),
                    "executable fixture return annotation"
                );
                ensure!(
                    functions.insert(node.name.as_str()),
                    "duplicate fixture function"
                );
            }
            _ => bail!("unexpected top-level fixture code; reviewed extraction update required"),
        }
    }
    ensure!(
        pytest && imports.len() == expected_imports.len() && declarations.len() == 2,
        "fixture imports/declarations missing"
    );
    ensure!(
        functions.len() == 33
            && functions.contains("_seed_claim_quote")
            && functions.contains("_preview"),
        "fixture function inventory changed; source review required"
    );
    for wanted in functions {
        ensure!(
            wanted.starts_with("test_") || matches!(wanted, "_preview" | "_seed_claim_quote"),
            "fixture function shadows a protected name"
        );
        let function = function(suite, wanted)?;
        for argument in function
            .args
            .posonlyargs
            .iter()
            .chain(&function.args.args)
            .chain(&function.args.kwonlyargs)
        {
            ensure!(
                matches!(
                    argument.def.arg.as_str(),
                    "hotkey"
                        | "accrued"
                        | "redeemable"
                        | "rows"
                        | "sold"
                        | "swept"
                        | "flushed"
                        | "substrate"
                        | "hotkeys"
                        | "payouts"
                        | "holdings"
                        | "networks"
                        | "name"
                        | "weight"
                        | "weight_fee"
                        | "limits"
                        | "reason"
                        | "hotkey_count"
                        | "expected_ok"
                        | "payout_failure"
                        | "monkeypatch"
                ),
                "fixture argument shadows a protected name"
            );
            ensure!(
                argument
                    .def
                    .annotation
                    .as_ref()
                    .is_none_or(|expr| annotation(expr)),
                "executable fixture argument annotation"
            );
            if let Some(default) = &argument.default {
                integer(default)?;
            }
        }
        if PARAMETRIZED.contains(&wanted) {
            continue;
        }
        ensure!(
            function
                .decorators
                .iter()
                .all(|expr| call_path(expr).as_deref() == Some("pytest.mark.asyncio")),
            "unrecognized decorator on {wanted}"
        );
    }
    Ok(())
}

fn indexed(expr: &ast::Expr) -> Result<u64> {
    let ast::Expr::ListComp(node) = expr else {
        bail!("indexed hotkeys must use the reviewed comprehension")
    };
    ensure!(
        node.generators.len() == 1,
        "indexed hotkeys generator count changed"
    );
    let generator = &node.generators[0];
    ensure!(
        !generator.is_async && generator.ifs.is_empty() && name(&generator.target, "i"),
        "indexed hotkeys target/filter changed"
    );
    let count = integer(&call(&generator.iter, "range", 1)?.args[0])?;
    ensure!(
        (1..=1024).contains(&count),
        "indexed hotkeys exceeds fixture bound"
    );
    let ast::Expr::JoinedStr(parts) = node.elt.as_ref() else {
        bail!("indexed hotkey format changed")
    };
    ensure!(
        parts.values.len() == 2 && string(&parts.values[0])? == "hotkey-",
        "indexed hotkey prefix changed"
    );
    let ast::Expr::FormattedValue(value) = &parts.values[1] else {
        bail!("indexed hotkey interpolation changed")
    };
    ensure!(
        name(&value.value, "i")
            && value.conversion.to_byte().is_none()
            && value.format_spec.is_none(),
        "indexed hotkey interpolation changed"
    );
    Ok(count)
}

fn key(expr: &ast::Expr) -> Result<&str> {
    let ast::Expr::Name(node) = expr else {
        bail!("fixture account must be a named dev-key role")
    };
    ensure!(
        matches!(node.id.as_str(), "ALICE" | "ALICE_HOT" | "BOB" | "BOB_HOT"),
        "unknown dev-key role"
    );
    Ok(node.id.as_str())
}

fn calls(body: &[ast::Stmt], wanted: &str) -> Result<Vec<ast::ExprCall>> {
    let mut result = Vec::new();
    for stmt in body {
        let expr = match stmt {
            ast::Stmt::Expr(node) => Some(&*node.value),
            ast::Stmt::Assign(node) => Some(&*node.value),
            _ => None,
        };
        if let Some(expr) = expr {
            effects::walk(expr, 0, &mut |expr| {
                if let ast::Expr::Call(node) = expr
                    && call_path(&node.func).as_deref() == Some(wanted)
                {
                    result.push(node.clone());
                }
            })?;
        }
    }
    ensure!(result.len() <= 32, "too many fixture calls");
    Ok(result)
}

fn fixture_scope(body: &[ast::Stmt]) -> Result<()> {
    let mut indexed_binding = false;
    for stmt in body {
        match stmt {
            ast::Stmt::Assign(node) => {
                for target in &node.targets {
                    let ast::Expr::Name(target) = target else {
                        continue;
                    };
                    ensure!(
                        !matches!(
                            target.id.as_str(),
                            "ALICE"
                                | "ALICE_HOT"
                                | "BOB"
                                | "BOB_HOT"
                                | "_preview"
                                | "_seed_claim_quote"
                                | "range"
                                | "RuntimeError"
                                | "fees"
                                | "pytest"
                        ),
                        "fixture shadows a projected name"
                    );
                    if target.id.as_str() == "hotkeys" {
                        ensure!(!indexed_binding, "indexed hotkeys rebound");
                        indexed(&node.value)?;
                        indexed_binding = true;
                    }
                }
            }
            ast::Stmt::Expr(node) => {
                let mut mutates = false;
                effects::walk(&node.value, 0, &mut |expr| {
                    if let ast::Expr::Call(call) = expr
                        && let ast::Expr::Attribute(method) = &*call.func
                        && name(&method.value, "hotkeys")
                    {
                        mutates = true;
                    }
                })?;
                ensure!(
                    !mutates,
                    "indexed hotkeys passed to an unrecognized mutation"
                );
                if let ast::Expr::Call(call) = &*node.value
                    && let ast::Expr::Attribute(method) = &*call.func
                    && name(&method.value, "hotkeys")
                {
                    bail!("indexed hotkeys mutated before fixture consumption");
                }
            }
            ast::Stmt::AugAssign(_)
            | ast::Stmt::AnnAssign(_)
            | ast::Stmt::Delete(_)
            | ast::Stmt::Global(_)
            | ast::Stmt::Nonlocal(_) => bail!("fixture mutation requires reviewed projection"),
            _ => {}
        }
    }
    Ok(())
}

fn keywords<'a>(
    node: &'a ast::ExprCall,
    required: &[&str],
    optional: &[&str],
) -> Result<BTreeMap<&'a str, &'a ast::Expr>> {
    let mut values = BTreeMap::new();
    for keyword in &node.keywords {
        let key = keyword
            .arg
            .as_ref()
            .context("fixture keyword unpacking unsupported")?
            .as_str();
        ensure!(
            (required.contains(&key) || optional.contains(&key))
                && values.insert(key, &keyword.value).is_none(),
            "extra/duplicate fixture keyword {key}"
        );
    }
    ensure!(
        required.iter().all(|name| values.contains_key(name)),
        "missing fixture keyword"
    );
    Ok(values)
}

fn key_numbers(expr: &ast::Expr) -> Result<Value> {
    let ast::Expr::Dict(node) = expr else {
        bail!("fixture key amounts must be a literal dict")
    };
    ensure!(node.keys.len() <= 4, "fixture key dictionary exceeds bound");
    let mut output = serde_json::Map::new();
    for (entry, value) in node.keys.iter().zip(&node.values) {
        let role = key(entry
            .as_ref()
            .context("fixture dict unpacking unsupported")?)?;
        ensure!(
            output.insert(role.into(), json!(integer(value)?)).is_none(),
            "duplicate fixture key"
        );
    }
    Ok(Value::Object(output))
}

fn seed(node: &ast::ExprCall) -> Result<Value> {
    ensure!(
        node.args.len() == 1 && name(&node.args[0], "substrate"),
        "seed positional input changed"
    );
    let values = keywords(node, &["hotkeys", "payouts", "holdings"], &["networks"])?;
    let keys = list(values["hotkeys"])?
        .iter()
        .map(key)
        .collect::<Result<Vec<_>>>()?;
    ensure!(!keys.is_empty(), "empty literal seed keys");
    let holdings = key_numbers(values["holdings"])?;
    ensure!(
        holdings
            .as_object()
            .context("holdings object")?
            .values()
            .all(|value| value.as_u64().is_some_and(|value| value <= 1024)),
        "fixture holding population exceeds bound"
    );
    let payouts = key_numbers(values["payouts"])?;
    ensure!(
        holdings.as_object().context("holdings object")?.len() == keys.len()
            && keys.iter().all(|key| holdings.get(*key).is_some())
            && payouts
                .as_object()
                .context("payouts object")?
                .keys()
                .all(|key| keys.contains(&key.as_str())),
        "seed hotkey mapping differs"
    );
    Ok(
        json!({"hotkeys":keys,"payouts":payouts,"holdings":holdings,"networks":values.get("networks").map(|expr| count(expr)).transpose()?.unwrap_or(1)}),
    )
}

fn preview(node: &ast::ExprCall) -> Result<Value> {
    ensure!(node.args.len() == 1, "preview positional inputs changed");
    let values = keywords(
        node,
        &["accrued", "redeemable", "rows", "sold"],
        &["swept", "flushed"],
    )?;
    let mut result = json!({"hotkey":key(&node.args[0])?,"swept":0,"flushed":0});
    for (key, value) in values {
        result[key] = json!(integer(value)?);
    }
    ensure!(
        ["rows", "sold", "swept", "flushed"]
            .iter()
            .all(|key| result[*key].as_u64().is_some_and(|value| value <= 1024)),
        "preview work exceeds fixture bound"
    );
    ensure!(
        result["accrued"].as_u64() >= result["redeemable"].as_u64()
            && result["rows"].as_u64() >= result["sold"].as_u64(),
        "negative preview remainder"
    );
    Ok(result)
}

fn repeated_amount(expr: &ast::Expr) -> Result<u64> {
    let ast::Expr::DictComp(node) = expr else {
        bail!("indexed fixture amounts must use the reviewed comprehension")
    };
    ensure!(
        node.generators.len() == 1,
        "indexed amount generator count changed"
    );
    let generator = &node.generators[0];
    ensure!(
        !generator.is_async
            && generator.ifs.is_empty()
            && name(&generator.target, "hotkey")
            && name(&generator.iter, "hotkeys")
            && name(&node.key, "hotkey"),
        "indexed amount comprehension changed"
    );
    integer(&node.value)
}

fn indexed_seed(suite: &[ast::Stmt], wanted: &str) -> Result<(ast::Expr, Value)> {
    let body = function(suite, wanted)?.body;
    fixture_scope(body)?;
    let count = indexed(assignment(body, "hotkeys")?)?;
    let matches = calls(body, "_seed_claim_quote")?;
    ensure!(
        matches.len() == 1,
        "{wanted}: indexed seed call count changed"
    );
    let node = &matches[0];
    ensure!(
        node.args.len() == 1 && name(&node.args[0], "substrate"),
        "indexed seed positional input changed"
    );
    let values = keywords(node, &["hotkeys", "payouts", "holdings", "networks"], &[])?;
    ensure!(
        name(values["hotkeys"], "hotkeys"),
        "indexed seed must consume the indexed hotkeys"
    );
    let holdings = repeated_amount(values["holdings"])?;
    ensure!(holdings <= 1024, "indexed holdings exceed fixture bound");
    Ok((
        ast::Expr::Call(node.clone()),
        json!({"hotkeys":count,"holdings":holdings,
        "payout":repeated_amount(values["payouts"])?,"networks":self::count(values["networks"])?}),
    ))
}

fn preview_shape(suite: &[ast::Stmt]) -> Result<()> {
    let helper = function(suite, "_preview")?;
    ensure!(
        helper.body.len() == 1
            && helper.args.args.len() == 1
            && helper.args.args[0].def.arg.as_str() == "hotkey"
            && helper.args.posonlyargs.is_empty()
            && helper.args.vararg.is_none()
            && helper.args.kwarg.is_none(),
        "preview helper signature/body changed"
    );
    let fields = helper
        .args
        .kwonlyargs
        .iter()
        .map(|arg| arg.def.arg.as_str())
        .collect::<Vec<_>>();
    ensure!(
        fields == ["accrued", "redeemable", "rows", "sold", "swept", "flushed"],
        "preview helper keyword order changed"
    );
    for (index, arg) in helper.args.kwonlyargs.iter().enumerate() {
        ensure!(
            if index < 4 {
                arg.default.is_none()
            } else {
                arg.default
                    .as_ref()
                    .is_some_and(|expr| integer(expr).ok() == Some(0))
            },
            "preview helper defaults changed"
        );
    }
    let ast::Stmt::Return(returned) = &helper.body[0] else {
        bail!("preview helper must directly return its record")
    };
    let ast::Expr::Dict(record) = returned
        .value
        .as_deref()
        .context("preview helper record missing")?
    else {
        bail!("preview helper must return a literal mapping")
    };
    let names = [
        "hotkey",
        "owed_shares",
        "accrued_tao",
        "redeemable_tao",
        "forfeited_tao_est",
        "rows",
        "rows_to_sell",
        "dust_rows",
        "swept",
        "flushed_credits",
    ];
    ensure!(
        record.keys.len() == names.len(),
        "preview helper fields changed"
    );
    for ((key, value), expected) in record.keys.iter().zip(&record.values).zip(names) {
        ensure!(
            string(key.as_ref().context("preview record unpacking")?)? == expected,
            "preview field order changed"
        );
        match expected {
            "owed_shares" => ensure!(integer(value)? == 1, "preview owed shares changed"),
            "forfeited_tao_est" | "dust_rows" => {
                let ast::Expr::BinOp(subtraction) = value else {
                    bail!("preview remainder changed")
                };
                let (left, right) = if expected == "dust_rows" {
                    ("rows", "sold")
                } else {
                    ("accrued", "redeemable")
                };
                ensure!(
                    subtraction.op == ast::Operator::Sub
                        && name(&subtraction.left, left)
                        && name(&subtraction.right, right),
                    "preview remainder changed"
                );
            }
            _ => {
                let field = match expected {
                    "accrued_tao" => "accrued",
                    "redeemable_tao" => "redeemable",
                    "rows_to_sell" => "sold",
                    "flushed_credits" => "flushed",
                    other => other,
                };
                ensure!(name(value, field), "preview field source changed");
            }
        }
    }
    Ok(())
}

fn network_range(expr: &ast::Expr, start: u64, value: bool) -> Result<u64> {
    let ast::Expr::ListComp(node) = expr else {
        bail!("network range shape changed")
    };
    ensure!(
        node.generators.len() == 1,
        "network generator count changed"
    );
    let generator = &node.generators[0];
    ensure!(
        !generator.is_async && generator.ifs.is_empty() && name(&generator.target, "netuid"),
        "network generator changed"
    );
    let items = tuple(&node.elt, 2)?;
    ensure!(
        name(&items[0], "netuid")
            && matches!(&items[1],ast::Expr::Constant(node) if node.value == ast::Constant::Bool(value)),
        "network row changed"
    );
    let range = call(&generator.iter, "range", if start == 0 { 1 } else { 2 })?;
    let end = if start == 0 {
        integer(&range.args[0])?
    } else {
        ensure!(
            integer(&range.args[0])? == start,
            "network ranges are not contiguous"
        );
        integer(&range.args[1])?
    };
    ensure!(
        end >= start && end <= 1024,
        "network range exceeds fixture bound"
    );
    Ok(end)
}

fn project(resolver: &mut Resolver<'_>, pins: &DocumentMut) -> Result<Value> {
    let test = resolver.source(TEST)?;
    let fees = resolver.source(FEES)?;
    let suite =
        ast::Suite::parse(&test, TEST).map_err(|error| anyhow::anyhow!("{TEST}: {error}"))?;
    envelope(&suite)?;
    preview_shape(&suite)?;
    let mut inputs = Vec::new();
    for (path, source) in [(TEST, &test), (FEES, &fees)] {
        let sha = hex::encode(Sha256::digest(source.as_bytes()));
        ensure!(
            pins["artifacts"][path]["to"].as_str()
                == Some(format!("vendor/upstream/src/{path}").as_str())
                && pins["artifacts"][path]["sha256"].as_str() == Some(sha.as_str()),
            "{path}: artifact identity differs"
        );
        inputs.push(json!({"path":path,"blob":resolver.tree.get(path),"sha256":sha}));
    }
    let values = resolver.values(FEES, CONSTANTS)?;
    let fee_suite =
        ast::Suite::parse(&fees, FEES).map_err(|error| anyhow::anyhow!("{FEES}: {error}"))?;
    let mut constants = serde_json::Map::new();
    for constant in CONSTANTS {
        let expr = assignment(&fee_suite, constant)?;
        let mut source = span(FEES, &fees, expr);
        source["symbol"] = json!(constant);
        let value = &values[*constant]["value"];
        ensure!(
            value.as_u64().is_some(),
            "{constant}: native constant changed"
        );
        constants.insert((*constant).into(), json!({"source":source,"value":value}));
    }
    let bindings = Bindings::from([("fees".into(), Binding::Module(FEES.into()))]);
    let node_expr = assignment(&suite, "_NODE_QUOTES")?;
    let node_param = parameter(
        &suite,
        PARAMETRIZED[0],
        &["name", "weight", "weight_fee", "limits"],
    )?;
    ensure!(
        name(node_param, "_NODE_QUOTES"),
        "node quote test no longer consumes the named table"
    );
    let mut nodes = Vec::new();
    for row in list(node_expr)? {
        let items = tuple(row, 4)?;
        let limits=list(&items[3])?.iter().map(|expr| {
            ensure!(matches!(expr,ast::Expr::Attribute(node) if name(&node.value,"fees") && matches!(node.attr.as_str(),"_MAX_ROOT_CLAIM_WORK"|"_MAX_ROOT_CLAIM_HOTKEY_WORK")),"node envelope reference changed");
            literal(resolver,&bindings,expr)
        }).collect::<Result<Vec<_>>>()?;
        ensure!(
            !limits.is_empty() && limits.len() <= 2,
            "node envelope count changed"
        );
        nodes.push(json!({"name":string(&items[0])?,"weight":integer(&items[1])?,"weight_fee":integer(&items[2])?,"limits":limits}));
    }
    ensure!(nodes.len() == 3, "node quote row count changed");
    let extension = assignment(&suite, "_EXTENSION_REF_TIME")?;
    let heavy = parameter(
        &suite,
        PARAMETRIZED[1],
        &["hotkeys", "holdings", "networks", "reason"],
    )?;
    let mut heavy_rows = Vec::new();
    for row in list(heavy)? {
        let items = tuple(row, 4)?;
        let count = if matches!(&items[0], ast::Expr::ListComp(_)) {
            indexed(&items[0])?
        } else {
            let keys = list(&items[0])?;
            ensure!(
                keys.len() == 1 && key(&keys[0])? == "ALICE_HOT",
                "heavy dev key changed"
            );
            1
        };
        heavy_rows.push(json!({"hotkeys":count,"holdings":self::count(&items[1])?,"networks":self::count(&items[2])?,"reason":string(&items[3])?}));
    }
    ensure!(heavy_rows.len() == 2, "heavy admission row count changed");
    let inclusive = parameter(
        &suite,
        PARAMETRIZED[4],
        &["hotkey_count", "holdings", "networks", "expected_ok"],
    )?;
    let mut inclusive_rows = Vec::new();
    for row in list(inclusive)? {
        let items = tuple(row, 4)?;
        let ast::Expr::Constant(boolean) = &items[3] else {
            bail!("inclusive outcome must be boolean")
        };
        let ast::Constant::Bool(allowed) = &boolean.value else {
            bail!("inclusive outcome must be boolean")
        };
        inclusive_rows.push(json!({"hotkeys":count(&items[0])?,"holdings":count(&items[1])?,"networks":count(&items[2])?,"allowed":allowed}));
    }
    ensure!(
        inclusive_rows.len() == 3,
        "inclusive admission row count changed"
    );
    let mut failures = serde_json::Map::new();
    for wanted in &PARAMETRIZED[2..4] {
        let expr = parameter(&suite, wanted, &["payout_failure"])?;
        let rows = list(expr)?;
        ensure!(rows.len() == 2, "payout failure row count changed");
        ensure!(
            matches!(&rows[0],ast::Expr::Constant(node) if node.value==ast::Constant::None),
            "first payout failure must be None"
        );
        let error = call(&rows[1], "RuntimeError", 1)?;
        failures.insert((*wanted).into(),record(&test,wanted,expr,json!([{"kind":"none"},{"kind":"error","type":"RuntimeError","message":string(&error.args[0])?}])));
    }
    let network_name = "test_network_count_does_not_affect_admission";
    let network = function(&suite, network_name)?;
    let count = indexed(assignment(network.body, "hotkeys")?)?;
    let map_calls = calls(network.body, "substrate.seed_map")?;
    ensure!(map_calls.len() == 1, "network map call missing/ambiguous");
    let map = &map_calls[0];
    ensure!(
        map.args.len() == 3
            && map.keywords.is_empty()
            && string(&map.args[0])? == "SubtensorModule"
            && string(&map.args[1])? == "NetworksAdded",
        "network map target changed"
    );
    let ast::Expr::BinOp(join) = &map.args[2] else {
        bail!("network ranges must concatenate")
    };
    ensure!(
        join.op == ast::Operator::Add,
        "network range operator changed"
    );
    let added = network_range(&join.left, 0, true)?;
    let end = network_range(&join.right, added, false)?;
    let (_, network_seed) = indexed_seed(&suite, network_name)?;
    ensure!(
        network_seed["hotkeys"] == json!(count) && network_seed["networks"] == json!(added),
        "network seed/population differs"
    );
    let shield_name = "test_admission_hard_stop_survives_unavailable_payout_preview";
    let (shield_expr, shield_case) = indexed_seed(&suite, shield_name)?;
    let mut seed_cases = serde_json::Map::new();
    for wanted in SEEDS {
        let body = function(&suite, wanted)?.body;
        fixture_scope(body)?;
        let matches = calls(body, "_seed_claim_quote")?;
        ensure!(matches.len() == 1, "{wanted}: seed call missing/ambiguous");
        let expr = ast::Expr::Call(matches[0].clone());
        seed_cases.insert(
            (*wanted).into(),
            record(&test, wanted, &expr, seed(&matches[0])?),
        );
    }
    // Case selection is explicit: additions cannot silently look extracted.
    ensure!(
        seed_cases.len() == SEEDS.len(),
        "seed fixture inventory differs"
    );
    let mut preview_cases = serde_json::Map::new();
    for wanted in PREVIEWS {
        let matches = calls(function(&suite, wanted)?.body, "_preview")?;
        ensure!(
            matches.len() == if *wanted == PREVIEWS[2] { 2 } else { 1 },
            "{wanted}: preview call count changed"
        );
        let rows = matches.iter().map(preview).collect::<Result<Vec<_>>>()?;
        let sources = matches
            .iter()
            .map(|node| span(TEST, &test, &ast::Expr::Call(node.clone())))
            .collect::<Vec<_>>();
        let expr = ast::Expr::Call(matches[0].clone());
        let mut projected = record(&test, wanted, &expr, json!(rows));
        projected["sources"] = json!(sources);
        preview_cases.insert((*wanted).into(), projected);
    }
    Ok(
        json!({"format":1,"provenance":{"tag":pins["parity"]["tag"].as_str(),"commit":pins["parity"]["commit"].as_str(),"inputs":inputs},
        "constants":constants,"extension_ref_time":record(&test,"_EXTENSION_REF_TIME",extension,json!(integer(extension)?)),
        "node_quotes":record(&test,"_NODE_QUOTES",node_expr,json!(nodes)),"admission_hard_stop":record(&test,PARAMETRIZED[1],heavy,json!(heavy_rows)),
        "admission_inclusive":record(&test,PARAMETRIZED[4],inclusive,json!(inclusive_rows)),"payout_failures":failures,
        "network_population":record(&test,network_name,&map.args[2],json!({"hotkeys":count,"added":added,"absent":end-added})),
        "shielded_heavy":record(&test,shield_name,&shield_expr,shield_case),
        "seed_cases":seed_cases,"preview_cases":preview_cases,
        "adaptations":["Dev-key names identify native fixture roles, not identical account bytes.","Native fees and account defaults remain adapted except where a projected case explicitly supplies them.","Inline arithmetic assertions and unprojected calls remain source-reviewed authored fixtures; this document does not claim all Python inputs were extracted."]}),
    )
}

pub(super) fn outputs(root: &Path, pins: &DocumentMut) -> Result<Vec<(&'static str, Vec<u8>)>> {
    let mut resolver = Resolver::new(root)?;
    Ok(vec![(
        OUTPUT,
        pretty(&project(&mut resolver, pins).context("root-claim fixture extraction")?)?,
    )])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn root() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_owned()
    }

    fn inspect(source: Option<String>) -> Result<Value> {
        let root = root();
        let mut pins = fs::read_to_string(root.join("upstream.toml"))?.parse::<DocumentMut>()?;
        let mut resolver = Resolver::new(&root)?;
        if let Some(source) = source {
            pins["artifacts"][TEST]["sha256"] =
                toml_edit::value(hex::encode(Sha256::digest(source.as_bytes())));
            resolver
                .tree
                .insert(TEST.into(), super::super::git_blob(source.as_bytes()));
            resolver.sources.insert(TEST.into(), source);
        }
        project(&mut resolver, &pins)
    }

    #[test]
    fn authenticated_root_claim_inputs_preserve_source_tables_and_cases() -> Result<()> {
        let document = inspect(None)?;
        assert_eq!(
            document["node_quotes"]["value"].as_array().unwrap().len(),
            3
        );
        assert_eq!(document["node_quotes"]["value"][0]["limits"], json!([129]));
        assert_eq!(
            document["node_quotes"]["value"][2]["weight_fee"],
            json!(16_481_677)
        );
        assert_eq!(
            document["admission_hard_stop"]["value"][0]["hotkeys"],
            json!(257)
        );
        assert_eq!(
            document["admission_hard_stop"]["value"][0]["networks"],
            json!(16)
        );
        assert_eq!(
            document["admission_inclusive"]["value"]
                .as_array()
                .unwrap()
                .len(),
            3
        );
        assert_eq!(document["constants"].as_object().unwrap().len(), 13);
        assert_eq!(
            document["seed_cases"].as_object().unwrap().len(),
            SEEDS.len()
        );
        assert_eq!(
            document["preview_cases"].as_object().unwrap().len(),
            PREVIEWS.len()
        );
        assert_eq!(
            document["shielded_heavy"]["value"],
            json!({"hotkeys":257,"holdings":0,"payout":1_000_000,"networks":16})
        );
        assert!(document["node_quotes"]["source"]["line"].as_u64().unwrap() > 0);
        Ok(())
    }

    #[test]
    fn rejects_fixture_mutation_and_unreviewed_executable_shapes() -> Result<()> {
        let original = fs::read_to_string(root().join("vendor/upstream/src").join(TEST))?;
        for (from, to) in [
            ("_root_claim_fee as fees", "_root_claim_fee as other"),
            ("import pytest", "import pytest as other"),
            (
                "_EXTENSION_REF_TIME = 60_000_000",
                "_EXTENSION_REF_TIME = 60_000_000\nfees = object()",
            ),
            ("_NODE_QUOTES = [", "_NODE_QUOTES = []\n_NODE_QUOTES = ["),
            (
                "@pytest.mark.parametrize((\"name\", \"weight\", \"weight_fee\", \"limits\"), _NODE_QUOTES)",
                "_NODE_QUOTES.reverse()\n@pytest.mark.parametrize((\"name\", \"weight\", \"weight_fee\", \"limits\"), _NODE_QUOTES)",
            ),
            (
                "(\"name\", \"weight\", \"weight_fee\", \"limits\"), _NODE_QUOTES",
                "(\"name\", \"weight\", \"weight_fee\", \"limits\"), []",
            ),
            ("fees._MAX_ROOT_CLAIM_HOTKEY_WORK]),", "fees.UNKNOWN]),"),
            ("for i in range(257)", "for i in range(257) if i"),
            ("for i in range(257)", "for i in range(1000000)"),
            ("f\"hotkey-{i}\"", "f\"hotkey-{i!r}\""),
            (
                "RuntimeError(\"payout unavailable\")",
                "RuntimeError(message=\"payout unavailable\")",
            ),
            (
                "async def test_reserved_fallback_is_max_root_claim_work():",
                "async def RuntimeError():",
            ),
            (
                "[None, RuntimeError(\"payout unavailable\")]",
                "[None, None]",
            ),
            (
                "hotkeys=[ALICE_HOT, BOB_HOT],",
                "hotkeys=[ALICE_HOT, BOB_HOT], unknown=1,",
            ),
            (
                "forfeited_tao_est\": accrued - redeemable",
                "forfeited_tao_est\": accrued + redeemable",
            ),
            (
                "for netuid in range(15, 40)",
                "for netuid in range(15, 40) if netuid",
            ),
        ] {
            ensure!(original.contains(from), "mutation needle missing: {from}");
            let changed = original.replace(from, to);
            assert!(
                inspect(Some(changed)).is_err(),
                "accepted mutation {from} -> {to}"
            );
        }
        Ok(())
    }

    #[test]
    fn literal_update_reextracts_and_artifact_hash_tampering_fails() -> Result<()> {
        let original = fs::read_to_string(root().join("vendor/upstream/src").join(TEST))?;
        let changed = original.replace(
            "8_224_263, [fees._MAX_ROOT_CLAIM_HOTKEY_WORK]",
            "8_224_264, [fees._MAX_ROOT_CLAIM_HOTKEY_WORK]",
        );
        ensure!(changed != original, "literal update needle missing");
        let document = inspect(Some(changed.clone()))?;
        assert_eq!(
            document["node_quotes"]["value"][0]["weight_fee"],
            json!(8_224_264)
        );
        let root = root();
        let pins = fs::read_to_string(root.join("upstream.toml"))?.parse::<DocumentMut>()?;
        let mut resolver = Resolver::new(&root)?;
        resolver.sources.insert(TEST.into(), changed);
        assert!(project(&mut resolver, &pins).is_err());
        Ok(())
    }
}
