// Derived from RaoFoundation/subtensor sdk/python/tests/unit/test_cli_null_pruning.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Bounded, nonexecuting projection of the reviewed pruning pytest inputs.

use super::{Resolver, call_path, line};
use anyhow::{Context, Result, bail, ensure};
use rustpython_parser::{
    Parse,
    ast::{self, Ranged},
};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub(super) const SOURCE: &str = "sdk/python/tests/unit/test_cli_null_pruning.py";
pub(super) const NAMES: &[&str] = &[
    "test_invalid_requests_never_submit.parametrize",
    "test_state_reads_are_pinned_and_match_admin_window.parametrize",
    "test_switch_probes_all_legacy_indices_and_owner_cooldown.inputs",
];
const TESTS: &[&str] = &[
    "test_continuation_and_cleanup_only_batches",
    "test_default_target_uses_shared_yuma_ceiling",
    "test_restart_resumes_pending_target_without_local_progress",
    "test_invalid_requests_never_submit",
    "test_waits_for_admin_window_without_submitting_failures",
    "test_admin_wait_timeout_and_interrupt_are_safe",
    "test_interrupt_while_waiting_can_resume",
    "test_interrupt_during_submission_blocks_uncertain_resubmission",
    "test_uncertain_receipt_does_not_retry",
    "test_permanent_failure_stops_and_cooldown_is_not_repeated",
    "test_window_race_waits_then_rechecks_state",
    "test_repeated_no_progress_stops",
    "test_lost_ownership_and_mechanism_changes_stop",
    "test_explicit_switch_waits_for_cooldown_and_verifies_mode",
    "test_dry_run_previews_one_batch_without_creating_broadcast_record",
    "test_batch_limit_allows_restart",
    "test_key_probe_never_loads_commit_values",
    "test_state_reads_are_pinned_and_match_admin_window",
    "test_switch_probes_all_legacy_indices_and_owner_cooldown",
    "test_cleanup_before_optional_switch",
    "test_incomplete_approval_does_not_repeat_transaction",
    "test_already_yuma_is_idempotent",
    "test_switch_queue_probe_works_through_client_backend",
];
const STATE_FIELDS: &[&str] = &[
    "block",
    "genesis",
    "owner",
    "population",
    "pending_target",
    "mechanisms",
    "minimum",
    "maximum",
    "mode",
    "frozen",
];

fn literal(expr: &ast::Expr, names: &BTreeMap<String, Value>, depth: usize) -> Result<Value> {
    ensure!(depth <= 16, "pruning fixture literal is too deep");
    Ok(match expr {
        ast::Expr::Constant(node) => match &node.value {
            ast::Constant::None => Value::Null,
            ast::Constant::Bool(value) => json!(value),
            ast::Constant::Str(value) => {
                ensure!(value.len() <= 4096, "fixture string too large");
                json!(value)
            }
            ast::Constant::Int(value) => json!(
                value
                    .to_string()
                    .parse::<u64>()
                    .context("fixture integer must fit u64")?
            ),
            _ => bail!("unsupported pruning fixture constant"),
        },
        ast::Expr::Name(node) => names
            .get(node.id.as_str())
            .context("unbound pruning fixture input")?
            .clone(),
        ast::Expr::Tuple(node) => {
            ensure!(node.elts.len() <= 128, "fixture tuple too large");
            Value::Array(
                node.elts
                    .iter()
                    .map(|value| literal(value, names, depth + 1))
                    .collect::<Result<_>>()?,
            )
        }
        ast::Expr::List(node) => {
            ensure!(node.elts.len() <= 128, "fixture list too large");
            Value::Array(
                node.elts
                    .iter()
                    .map(|value| literal(value, names, depth + 1))
                    .collect::<Result<_>>()?,
            )
        }
        ast::Expr::Dict(node) => {
            ensure!(
                node.keys.len() <= 128 && node.keys.len() == node.values.len(),
                "fixture dictionary bounds"
            );
            let mut out = Map::new();
            for (key, value) in node.keys.iter().zip(&node.values) {
                let key = literal(
                    key.as_ref().context("fixture dictionary unpacking")?,
                    names,
                    depth + 1,
                )?;
                let key = key
                    .as_str()
                    .context("fixture dictionary key must be a string")?;
                ensure!(
                    out.insert(key.into(), literal(value, names, depth + 1)?)
                        .is_none(),
                    "duplicate fixture dictionary key"
                );
            }
            Value::Object(out)
        }
        _ => bail!("pruning fixture input must be literal or a bound parameter"),
    })
}
fn named<'a>(expr: &'a ast::Expr, wanted: &str) -> Result<&'a ast::ExprCall> {
    let ast::Expr::Call(call) = expr else {
        bail!("fixture constructor is not a call")
    };
    ensure!(
        call_path(&call.func).as_deref() == Some(wanted) && call.args.is_empty(),
        "changed fixture constructor"
    );
    let mut names = BTreeSet::new();
    for key in &call.keywords {
        ensure!(
            names.insert(key.arg.as_ref().context("constructor unpacking")?.as_str()),
            "duplicate constructor field"
        );
    }
    Ok(call)
}
fn keyword<'a>(call: &'a ast::ExprCall, wanted: &str) -> Result<&'a ast::Expr> {
    call.keywords
        .iter()
        .find(|value| {
            value
                .arg
                .as_ref()
                .is_some_and(|name| name.as_str() == wanted)
        })
        .map(|value| &value.value)
        .context("missing fixture keyword")
}
fn assignment<'a>(body: &'a [ast::Stmt], wanted: &str) -> Result<&'a ast::Expr> {
    let mut matches = body.iter().filter_map(|stmt| match stmt {
        ast::Stmt::Assign(node)
            if node.targets.len() == 1
                && matches!(&node.targets[0],ast::Expr::Name(name) if name.id.as_str()==wanted) =>
        {
            Some(&*node.value)
        }
        _ => None,
    });
    let result = matches.next().context("missing fixture assignment")?;
    ensure!(matches.next().is_none(), "rebound fixture assignment");
    Ok(result)
}
fn arguments(args: &ast::Arguments, expected: &[&str]) -> Result<()> {
    ensure!(
        args.posonlyargs.is_empty()
            && args.kwonlyargs.is_empty()
            && args.vararg.is_none()
            && args.kwarg.is_none()
            && args.args.iter().all(|arg| arg.default.is_none())
            && args
                .args
                .iter()
                .map(|arg| arg.def.arg.as_str())
                .eq(expected.iter().copied()),
        "changed pruning test parameter schema"
    );
    Ok(())
}
fn inert_defaults(args: &ast::Arguments) -> Result<()> {
    for arg in args
        .posonlyargs
        .iter()
        .chain(&args.args)
        .chain(&args.kwonlyargs)
    {
        ensure!(
            arg.def.annotation.is_none(),
            "executable fixture argument annotation"
        );
        if let Some(value) = &arg.default {
            literal(value, &BTreeMap::new(), 0)?;
        }
    }
    Ok(())
}
fn state_schema(value: &Map<String, Value>) -> Result<()> {
    ensure!(
        value.len() == STATE_FIELDS.len(),
        "state field count changed"
    );
    for (name, value) in value {
        let valid = match name.as_str() {
            "genesis" | "owner" | "mode" => value.is_string(),
            "frozen" => value.is_boolean(),
            "pending_target" => value.is_null() || value.is_u64(),
            "block" | "population" | "mechanisms" | "minimum" | "maximum" => value.is_u64(),
            _ => false,
        };
        ensure!(valid, "state input scalar type changed");
    }
    Ok(())
}
fn baseline(function: &ast::StmtFunctionDef) -> Result<Value> {
    ensure!(
        function.decorator_list.is_empty()
            && function.type_params.is_empty()
            && function.args.args.is_empty()
            && function.args.posonlyargs.is_empty()
            && function.args.kwonlyargs.is_empty()
            && function.args.vararg.is_none()
            && function
                .args
                .kwarg
                .as_ref()
                .is_some_and(|arg| arg.arg.as_str() == "changes")
            && function.body.len() == 2,
        "changed pruning state helper"
    );
    let ast::Stmt::Assign(initial) = &function.body[0] else {
        bail!("changed state assignment")
    };
    ensure!(
        initial.targets.len() == 1
            && matches!(&initial.targets[0],ast::Expr::Name(name) if name.id.as_str()=="initial"),
        "changed state binding"
    );
    let call = named(&initial.value, "pruning.PruningState")?;
    ensure!(
        call.keywords
            .iter()
            .map(|key| key.arg.as_ref().map(|name| name.as_str()))
            .eq(STATE_FIELDS.iter().map(|name| Some(*name))),
        "changed state fields"
    );
    let mut out = Map::new();
    for key in &call.keywords {
        out.insert(
            key.arg.as_ref().context("state field")?.to_string(),
            literal(&key.value, &BTreeMap::new(), 0)?,
        );
    }
    let ast::Stmt::Return(ret) = &function.body[1] else {
        bail!("changed state return")
    };
    let ast::Expr::Call(call) = ret.value.as_deref().context("state return")? else {
        bail!("changed state return call")
    };
    ensure!(
        call_path(&call.func).as_deref() == Some("replace")
            && call.args.len() == 1
            && matches!(&call.args[0],ast::Expr::Name(name) if name.id.as_str()=="initial")
            && call.keywords.len() == 1
            && call.keywords[0].arg.is_none()
            && matches!(&call.keywords[0].value,ast::Expr::Name(name) if name.id.as_str()=="changes"),
        "changed state replacement semantics"
    );
    state_schema(&out)?;
    Ok(Value::Object(out))
}
fn decorator<'a>(function: &'a ast::StmtFunctionDef, expected: &[&str]) -> Result<&'a ast::Expr> {
    ensure!(
        function.decorator_list.len() == 1,
        "changed pruning test decorators"
    );
    let ast::Expr::Call(call) = &function.decorator_list[0] else {
        bail!("test decorator is not a call")
    };
    ensure!(
        call_path(&call.func).as_deref() == Some("pytest.mark.parametrize")
            && call.args.len() == 2
            && call.keywords.is_empty(),
        "changed pytest parametrization"
    );
    ensure!(
        literal(&call.args[0], &BTreeMap::new(), 0)?.as_str() == Some(expected.join(",").as_str()),
        "changed pytest parameter names"
    );
    ensure!(
        matches!(&call.args[1], ast::Expr::List(_)),
        "pytest cases must be a literal list"
    );
    Ok(&call.args[1])
}
fn projected(
    source: &str,
    function: &ast::StmtFunctionDef,
    expression: &ast::Expr,
    value: Value,
) -> Value {
    json!({"line":line(source,expression.range().start().to_usize()),"end_line":line(source,expression.range().end().to_usize()),"value":value,
        "derived":{"kind":"pruning-pytest-inputs","function":function.name.as_str(),"function_line":line(source,function.range.start().to_usize()),"function_end_line":line(source,function.range.end().to_usize())}})
}
fn exact_keywords(call: &ast::ExprCall, expected: &[&str]) -> Result<()> {
    ensure!(
        call.keywords
            .iter()
            .map(|key| key.arg.as_ref().map(|name| name.as_str()))
            .eq(expected.iter().map(|name| Some(*name))),
        "fixture constructor field schema/order changed"
    );
    Ok(())
}
fn query_helper(function: &ast::StmtFunctionDef) -> Result<()> {
    let mut queries = function.body.iter().filter_map(|stmt| match stmt {
        ast::Stmt::AsyncFunctionDef(node) => Some(node),
        _ => None,
    });
    let query = queries.next().context("missing fixture query helper")?;
    ensure!(
        queries.next().is_none()
            && query.name.as_str() == "query"
            && query.body.len() == 1
            && query.decorator_list.is_empty()
            && query.type_params.is_empty()
            && query.returns.is_none(),
        "changed fixture query helper"
    );
    ensure!(query.args.posonlyargs.is_empty() && query.args.kwonlyargs.is_empty() && query.args.vararg.is_none() && query.args.kwarg.is_none() && query.args.args.len()==2
        && query.args.args[0].def.arg.as_str()=="item" && query.args.args[0].default.is_none() && query.args.args[1].def.arg.as_str()=="_params"
        && query.args.args[1].default.as_deref().is_some_and(|expr|matches!(expr,ast::Expr::Constant(value) if matches!(&value.value,ast::Constant::None))),"changed query argument semantics");
    inert_defaults(&query.args)?;
    let ast::Stmt::Return(ret) = &query.body[0] else {
        bail!("query no longer returns storage")
    };
    let ast::Expr::Subscript(value) = ret.value.as_deref().context("query return")? else {
        bail!("query storage lookup changed")
    };
    ensure!(
        matches!(&*value.value,ast::Expr::Name(name) if name.id.as_str()=="values")
            && matches!(&*value.slice,ast::Expr::Attribute(attribute) if attribute.attr.as_str()=="name" && matches!(&*attribute.value,ast::Expr::Name(name) if name.id.as_str()=="item")),
        "query no longer reads its input dictionary"
    );
    Ok(())
}
fn query_mock(expression: &ast::Expr) -> Result<()> {
    let call = named(expression, "AsyncMock")?;
    exact_keywords(call, &["side_effect"])?;
    ensure!(
        matches!(keyword(call,"side_effect")?,ast::Expr::Name(name) if name.id.as_str()=="query"),
        "query mock no longer uses its storage helper"
    );
    Ok(())
}
fn storage_keys(expression: &ast::Expr, expected: &[&str]) -> Result<Value> {
    let ast::Expr::Dict(dictionary) = expression else {
        bail!("storage producer is not a literal dictionary")
    };
    let mut keys = Vec::new();
    for key in &dictionary.keys {
        let ast::Expr::Constant(value) = key.as_ref().context("storage producer unpacking")? else {
            bail!("storage keys must be literal strings")
        };
        let ast::Constant::Str(value) = &value.value else {
            bail!("storage key is not a string")
        };
        keys.push(value.as_str());
    }
    ensure!(
        keys.iter().copied().eq(expected.iter().copied()),
        "storage producer field order/schema changed"
    );
    Ok(json!(keys))
}
fn read_invocation(expression: &ast::Expr, expected: &str) -> Result<Value> {
    let ast::Expr::Call(outer) = expression else {
        bail!("fixture run wrapper changed")
    };
    ensure!(
        call_path(&outer.func).as_deref() == Some("asyncio.run")
            && outer.args.len() == 1
            && outer.keywords.is_empty(),
        "fixture no longer runs reviewed async reader"
    );
    let ast::Expr::Call(inner) = &outer.args[0] else {
        bail!("fixture reader invocation changed")
    };
    ensure!(
        call_path(&inner.func).as_deref() == Some(expected)
            && inner.args.len() == 2
            && inner.keywords.is_empty()
            && matches!(&inner.args[0],ast::Expr::Name(name) if name.id.as_str()=="client"),
        "fixture reader target changed"
    );
    let netuid = literal(&inner.args[1], &BTreeMap::new(), 0)?;
    ensure!(netuid.is_u64(), "fixture netuid is not a literal integer");
    Ok(netuid)
}
fn invalid_consumer(function: &ast::StmtFunctionDef) -> Result<()> {
    ensure!(function.body.len() == 3, "invalid-request consumer changed");
    let app = assignment(&function.body, "app")?;
    let ast::Expr::Call(app) = app else {
        bail!("invalid app constructor")
    };
    ensure!(
        call_path(&app.func).as_deref() == Some("App")
            && app.keywords.is_empty()
            && app.args.len() == 1
            && matches!(&app.args[0],ast::Expr::Name(name) if name.id.as_str()=="initial"),
        "invalid app no longer receives initial state"
    );
    let ast::Stmt::With(context) = &function.body[1] else {
        bail!("invalid-request refusal context changed")
    };
    ensure!(
        context.items.len() == 1
            && context.body.len() == 1
            && context.items[0].optional_vars.is_none(),
        "invalid-request exception context changed"
    );
    let ast::Expr::Call(raises) = &context.items[0].context_expr else {
        bail!("invalid exception producer")
    };
    ensure!(
        call_path(&raises.func).as_deref() == Some("pytest.raises")
            && raises.args.len() == 1
            && matches!(&raises.args[0],ast::Expr::Name(name) if name.id.as_str()=="ValueError"),
        "invalid exception type changed"
    );
    exact_keywords(raises, &["match"])?;
    ensure!(
        matches!(keyword(raises,"match")?,ast::Expr::Name(name) if name.id.as_str()=="message"),
        "invalid diagnostic no longer consumes input"
    );
    let ast::Stmt::Expr(run) = &context.body[0] else {
        bail!("invalid request invocation")
    };
    let ast::Expr::Call(run) = &*run.value else {
        bail!("invalid request invocation")
    };
    ensure!(
        call_path(&run.func).as_deref() == Some("run")
            && run.args.len() == 2
            && matches!(&run.args[0],ast::Expr::Name(name) if name.id.as_str()=="app")
            && matches!(&run.args[1],ast::Expr::Name(name) if name.id.as_str()=="tmp_path"),
        "invalid request no longer uses reviewed app"
    );
    exact_keywords(run, &["target"])?;
    ensure!(
        matches!(keyword(run,"target")?,ast::Expr::Name(name) if name.id.as_str()=="target"),
        "invalid request no longer uses target input"
    );
    let ast::Stmt::Assert(assertion) = &function.body[2] else {
        bail!("invalid no-submission assertion changed")
    };
    let ast::Expr::Compare(compare) = &*assertion.test else {
        bail!("invalid no-submission comparison")
    };
    ensure!(
        call_path(&compare.left).as_deref() == Some("app.calls")
            && compare.ops == vec![ast::CmpOp::Eq]
            && compare.comparators.len() == 1
            && matches!(&compare.comparators[0],ast::Expr::List(list) if list.elts.is_empty()),
        "invalid no-submission assertion changed"
    );
    Ok(())
}
fn expected_properties(body: &[ast::Stmt], bindings: &BTreeMap<String, Value>) -> Result<Value> {
    let mut out = Map::new();
    for statement in body {
        let ast::Stmt::Assert(assertion) = statement else {
            continue;
        };
        let ast::Expr::Compare(compare) = &*assertion.test else {
            bail!("changed window assertion")
        };
        ensure!(
            compare.ops.len() == 1
                && compare.comparators.len() == 1
                && matches!(compare.ops[0], ast::CmpOp::Eq | ast::CmpOp::Is),
            "changed window assertion operator"
        );
        let ast::Expr::Attribute(attribute) = &*compare.left else {
            bail!("changed window assertion property")
        };
        ensure!(
            matches!(&*attribute.value,ast::Expr::Name(name) if name.id.as_str()=="actual"),
            "changed window assertion owner"
        );
        ensure!(
            out.insert(
                attribute.attr.to_string(),
                literal(&compare.comparators[0], bindings, 0)?
            )
            .is_none(),
            "duplicate window assertion"
        );
    }
    ensure!(
        out.keys().map(String::as_str).collect::<BTreeSet<_>>()
            == BTreeSet::from(["frozen", "yuma_ceiling", "pending_target", "mode"]),
        "changed window assertion inventory"
    );
    Ok(Value::Object(out))
}
fn import_names(names: &[ast::Alias]) -> String {
    names
        .iter()
        .map(|name| match &name.asname {
            Some(alias) => format!("{}={alias}", name.name),
            None => name.name.to_string(),
        })
        .collect::<Vec<_>>()
        .join(",")
}
pub(super) fn values(resolver: &mut Resolver<'_>, names: &[&str]) -> Result<Value> {
    ensure!(names == NAMES, "changed pruning input selection");
    let source = resolver.source(SOURCE)?;
    let suite =
        ast::Suite::parse(&source, SOURCE).map_err(|error| anyhow::anyhow!("{SOURCE}: {error}"))?;
    let mut imports = Vec::new();
    let mut functions = BTreeMap::new();
    let mut aliases = BTreeSet::new();
    let mut app = false;
    let mut declarations = false;
    for stmt in &suite {
        match stmt {
            ast::Stmt::Import(node) => {
                ensure!(!declarations, "late fixture import");
                imports.push(format!("import:{}", import_names(&node.names)));
            }
            ast::Stmt::ImportFrom(node) => {
                ensure!(!declarations, "late fixture import");
                imports.push(format!(
                    "from:{}:{}:{}",
                    node.level.map_or(0, |level| level.to_usize()),
                    node.module.as_ref().map_or("", |name| name.as_str()),
                    import_names(&node.names)
                ));
            }
            ast::Stmt::Assign(node) => {
                declarations = true;
                ensure!(node.targets.len() == 1, "fixture alias target");
                let ast::Expr::Name(name) = &node.targets[0] else {
                    bail!("fixture alias")
                };
                let target = match name.id.as_str() {
                    "READ_STATE" => "pruning.read_state",
                    "SWITCH_WAIT_REASON" => "pruning.switch_wait_reason",
                    _ => bail!("unreviewed fixture module binding"),
                };
                ensure!(
                    aliases.insert(name.id.as_str())
                        && call_path(&node.value).as_deref() == Some(target),
                    "rebound fixture producer"
                );
            }
            ast::Stmt::FunctionDef(node) => {
                declarations = true;
                ensure!(
                    functions.insert(node.name.as_str(), node).is_none()
                        && node.type_params.is_empty()
                        && node.returns.is_none(),
                    "duplicate fixture function"
                );
                inert_defaults(&node.args)?;
                if !matches!(
                    node.name.as_str(),
                    "test_invalid_requests_never_submit"
                        | "test_state_reads_are_pinned_and_match_admin_window"
                        | "reads"
                ) {
                    ensure!(
                        node.decorator_list.is_empty(),
                        "unreviewed fixture decorator"
                    );
                }
                if node.name.as_str() == "reads" {
                    ensure!(node.decorator_list.len() == 1, "fixture decorator");
                    let call = named(&node.decorator_list[0], "pytest.fixture")?;
                    ensure!(
                        call.keywords.len() == 1
                            && literal(keyword(call, "autouse")?, &BTreeMap::new(), 0)?
                                == json!(true),
                        "changed automatic fixture"
                    );
                }
            }
            ast::Stmt::ClassDef(node) => {
                declarations = true;
                ensure!(
                    !app && node.name.as_str() == "App"
                        && node.bases.is_empty()
                        && node.keywords.is_empty()
                        && node.decorator_list.is_empty()
                        && node.type_params.is_empty(),
                    "changed fixture class"
                );
                app = true;
                let mut methods = BTreeSet::new();
                for statement in &node.body {
                    let ast::Stmt::FunctionDef(method) = statement else {
                        bail!("executable fixture class body")
                    };
                    ensure!(
                        methods.insert(method.name.as_str())
                            && method.decorator_list.is_empty()
                            && method.type_params.is_empty()
                            && method.returns.is_none(),
                        "changed fixture class declaration"
                    );
                    inert_defaults(&method.args)?;
                }
                ensure!(
                    methods
                        == BTreeSet::from([
                            "__init__",
                            "run",
                            "resolve_address",
                            "resolve_dispatch_proxy",
                            "submit"
                        ]),
                    "changed fixture class methods"
                );
            }
            ast::Stmt::Expr(node) if matches!(&*node.value,ast::Expr::Constant(value) if matches!(&value.value,ast::Constant::Str(_))) =>
                {}
            _ => bail!("executable/unreviewed pruning fixture module statement"),
        }
    }
    ensure!(
        imports.iter().map(String::as_str).eq([
            "import:asyncio",
            "from:0:dataclasses:replace",
            "from:0:types:SimpleNamespace",
            "from:0:unittest.mock:AsyncMock,Mock",
            "import:pytest",
            "import:typer",
            "from:0:bittensor.cli:null_pruning=pruning",
            "from:0:bittensor.result:ChainError,ExtrinsicResult"
        ]),
        "changed pruning producer imports"
    );
    ensure!(
        app && aliases.len() == 2
            && functions.keys().copied().collect::<BTreeSet<_>>()
                == TESTS
                    .iter()
                    .copied()
                    .chain(["state", "receipt", "reads", "run"])
                    .collect(),
        "changed pruning function inventory"
    );
    let state_fn = functions["state"];
    let initial = baseline(state_fn)?;
    let invalid = functions["test_invalid_requests_never_submit"];
    arguments(&invalid.args, &["tmp_path", "initial", "target", "message"])?;
    invalid_consumer(invalid)?;
    let expression = decorator(invalid, &["initial", "target", "message"])?;
    let ast::Expr::List(list) = expression else {
        bail!("invalid cases")
    };
    ensure!(
        !list.elts.is_empty() && list.elts.len() <= 128,
        "invalid case bounds"
    );
    let mut rejected = Vec::new();
    for row in &list.elts {
        let ast::Expr::Tuple(row) = row else {
            bail!("invalid request row")
        };
        ensure!(row.elts.len() == 3, "invalid request row width");
        let call = named(&row.elts[0], "state")?;
        let mut state = initial.as_object().context("initial state object")?.clone();
        for keyword in &call.keywords {
            let name = keyword.arg.as_ref().context("state unpacking")?.as_str();
            ensure!(STATE_FIELDS.contains(&name), "unknown state override");
            state.insert(name.into(), literal(&keyword.value, &BTreeMap::new(), 0)?);
        }
        state_schema(&state)?;
        let target = literal(&row.elts[1], &BTreeMap::new(), 0)?;
        let message = literal(&row.elts[2], &BTreeMap::new(), 0)?;
        ensure!(
            target.is_u64() && message.is_string(),
            "invalid request carrier"
        );
        rejected.push(json!({"state":state,"target":target,"message":message}));
    }
    let mut rejection = projected(&source, invalid, expression, json!(rejected));
    rejection["derived"]["helper_line"] = json!(line(&source, state_fn.range.start().to_usize()));
    rejection["derived"]["helper_end_line"] = json!(line(&source, state_fn.range.end().to_usize()));
    let windows = functions["test_state_reads_are_pinned_and_match_admin_window"];
    arguments(
        &windows.args,
        &["tempo", "last", "pending", "window", "frozen"],
    )?;
    ensure!(windows.body.len() == 10, "changed window fixture body");
    let ast::Stmt::Expr(last) = &windows.body[9] else {
        bail!("window pin assertion changed")
    };
    let ast::Expr::Call(last) = &*last.value else {
        bail!("window pin assertion changed")
    };
    ensure!(
        call_path(&last.func).as_deref() == Some("client.at.assert_awaited_once")
            && last.args.is_empty()
            && last.keywords.is_empty(),
        "window pin assertion changed"
    );
    let expression = decorator(windows, &["tempo", "last", "pending", "window", "frozen"])?;
    let ast::Expr::List(list) = expression else {
        bail!("window rows")
    };
    ensure!(
        !list.elts.is_empty() && list.elts.len() <= 128,
        "window case bounds"
    );
    let window_netuid = read_invocation(assignment(&windows.body, "actual")?, "READ_STATE")?;
    query_helper(windows)?;
    let storage = assignment(&windows.body, "values")?;
    let view = named(assignment(&windows.body, "view")?, "SimpleNamespace")?;
    exact_keywords(view, &["block", "query"])?;
    query_mock(keyword(view, "query")?)?;
    let window_client = named(assignment(&windows.body, "client")?, "SimpleNamespace")?;
    exact_keywords(window_client, &["at"])?;
    let window_at = named(keyword(window_client, "at")?, "AsyncMock")?;
    exact_keywords(window_at, &["return_value"])?;
    ensure!(
        matches!(keyword(window_at,"return_value")?,ast::Expr::Name(name) if name.id.as_str()=="view"),
        "window client no longer receives its pinned view"
    );
    let block = literal(keyword(view, "block")?, &BTreeMap::new(), 0)?;
    ensure!(block.is_u64(), "window block integer");
    let mut rows = Vec::new();
    let storage_order = storage_keys(
        storage,
        &[
            "NetworksAdded",
            "SubnetOwner",
            "SubnetworkN",
            "NullPruningTarget",
            "MechanismCountCurrent",
            "MinAllowedUids",
            "MaxAllowedUids",
            "SubnetEpochConsensus",
            "Tempo",
            "LastEpochBlock",
            "PendingEpochAt",
            "AdminFreezeWindow",
            "BlockHash",
        ],
    )?;
    for row in &list.elts {
        let ast::Expr::Tuple(row) = row else {
            bail!("window row")
        };
        ensure!(row.elts.len() == 5, "window row width");
        let values = row
            .elts
            .iter()
            .map(|value| literal(value, &BTreeMap::new(), 0))
            .collect::<Result<Vec<_>>>()?;
        ensure!(
            values[..4].iter().all(Value::is_u64) && values[4].is_boolean(),
            "window input types"
        );
        let bindings: BTreeMap<String, Value> = ["tempo", "last", "pending", "window", "frozen"]
            .into_iter()
            .zip(values)
            .map(|(name, value)| (name.into(), value))
            .collect();
        rows.push(json!({"block":block,"netuid":window_netuid,"storage":literal(storage,&bindings,0)?,"expected":expected_properties(&windows.body,&bindings)?}));
    }
    let mut window_rows = projected(&source, windows, expression, json!(rows));
    window_rows["derived"]["storage_line"] =
        json!(line(&source, storage.range().start().to_usize()));
    window_rows["derived"]["storage_end_line"] =
        json!(line(&source, storage.range().end().to_usize()));
    window_rows["derived"]["storage_keys"] = storage_order;
    let switch = functions["test_switch_probes_all_legacy_indices_and_owner_cooldown"];
    arguments(&switch.args, &[])?;
    ensure!(switch.body.len() == 10, "changed switch fixture body");
    let ast::Stmt::Assert(first_assert) = &switch.body[4] else {
        bail!("switch wait assertion changed")
    };
    let ast::Expr::Compare(first_compare) = &*first_assert.test else {
        bail!("switch wait comparison changed")
    };
    ensure!(
        first_compare.ops == vec![ast::CmpOp::Eq]
            && first_compare.comparators.len() == 1
            && literal(&first_compare.comparators[0], &BTreeMap::new(), 0)?.is_string(),
        "switch wait assertion carrier changed"
    );
    let switch_netuid = read_invocation(&first_compare.left, "SWITCH_WAIT_REASON")?;
    query_helper(switch)?;
    let storage = assignment(&switch.body, "values")?;
    let client = named(assignment(&switch.body, "client")?, "SimpleNamespace")?;
    exact_keywords(client, &["_substrate", "block", "at"])?;
    ensure!(
        matches!(keyword(client,"_substrate")?,ast::Expr::Name(name) if name.id.as_str()=="substrate"),
        "changed switch substrate producer"
    );
    let block_mock = named(keyword(client, "block")?, "AsyncMock")?;
    exact_keywords(block_mock, &["return_value"])?;
    let block = literal(keyword(block_mock, "return_value")?, &BTreeMap::new(), 0)?;
    let at_mock = named(keyword(client, "at")?, "AsyncMock")?;
    exact_keywords(at_mock, &["return_value"])?;
    let view = named(keyword(at_mock, "return_value")?, "SimpleNamespace")?;
    exact_keywords(view, &["block", "query"])?;
    query_mock(keyword(view, "query")?)?;
    ensure!(
        block.is_u64() && block == literal(keyword(view, "block")?, &BTreeMap::new(), 0)?,
        "switch block producers disagree"
    );
    let mut switch_inputs = projected(
        &source,
        switch,
        storage,
        json!({"block":block,"netuid":switch_netuid,"storage":literal(storage,&BTreeMap::new(),0)?}),
    );
    switch_inputs["derived"]["storage_keys"] = storage_keys(
        storage,
        &["LastRateLimitedBlock", "Tempo", "OwnerHyperparamRateLimit"],
    )?;
    let output = Value::Object(Map::from_iter([
        (NAMES[0].into(), rejection),
        (NAMES[1].into(), window_rows),
        (NAMES[2].into(), switch_inputs),
    ]));
    let bytes = serde_json::to_vec(&output)?;
    ensure!(bytes.len() <= 65_536, "pruning fixture output exceeds64KiB");
    super::spend(&resolver.budget, bytes.len())?;
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    fn root() -> Result<PathBuf> {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .context("workspace root")
            .map(PathBuf::from)
    }
    fn extract(override_source: Option<String>) -> Result<Value> {
        let root = root()?;
        let pins = std::fs::read_to_string(root.join("upstream.toml"))?.parse()?;
        let mut resolver = Resolver::new(&root)?;
        if let Some(source) = override_source {
            resolver.sources.insert(SOURCE.into(), source);
        }
        super::super::document(&mut resolver, &pins, SOURCE, NAMES)
    }
    #[test]
    fn pruning_pytest_inputs_keep_authenticated_producers_and_spans() -> Result<()> {
        let actual = extract(None)?;
        let root = root()?;
        let resolver = Resolver::new(&root)?;
        ensure!(
            actual["provenance"]["path"] == SOURCE
                && actual["provenance"]["blob"] == resolver.tree[SOURCE]
        );
        ensure!(
            actual["values"][NAMES[0]]["value"]
                .as_array()
                .context("invalid rows")?
                .len()
                == 4
        );
        ensure!(
            actual["values"][NAMES[1]]["value"]
                .as_array()
                .context("window rows")?
                .len()
                == 5
        );
        for name in NAMES {
            let value = &actual["values"][*name];
            ensure!(
                value["line"].as_u64().context("line")? > 0
                    && value["end_line"].as_u64() >= value["line"].as_u64()
            );
            ensure!(value["derived"]["kind"] == "pruning-pytest-inputs");
        }
        Ok(())
    }
    #[test]
    fn pruning_pytest_literal_changes_flow_to_native_inputs() -> Result<()> {
        let root = root()?;
        let resolver = Resolver::new(&root)?;
        let source = resolver.source(SOURCE)?;
        let changed = source
            .replace("minimum=64,", "minimum=65,")
            .replace("(100, 50, 0, 20, False)", "(100, 51, 0, 20, False)")
            .replace(
                "\"LastRateLimitedBlock\": 90",
                "\"LastRateLimitedBlock\": 91",
            );
        ensure!(changed != source);
        let actual = extract(Some(changed))?;
        ensure!(actual["values"][NAMES[0]]["value"][0]["state"]["minimum"] == 65);
        ensure!(actual["values"][NAMES[1]]["value"][0]["storage"]["LastEpochBlock"] == 51);
        ensure!(actual["values"][NAMES[2]]["value"]["storage"]["LastRateLimitedBlock"] == 91);
        Ok(())
    }
    #[test]
    fn pruning_pytest_producers_refuse_semantic_drift_and_dynamic_inputs() -> Result<()> {
        let root = root()?;
        let resolver = Resolver::new(&root)?;
        let source = resolver.source(SOURCE)?;
        for changed in [
            source.replace("return replace(initial, **changes)", "return initial"),
            source.replace("return values[item.name]", "return {}"),
            source.replace(
                "actual = asyncio.run(READ_STATE(client, 1))",
                "actual = opaque(values)",
            ),
            source.replace(
                "run(app, tmp_path, target=target)",
                "run(app, tmp_path, target=1)",
            ),
            source.replace("client.at.assert_awaited_once()", "values.clear()"),
            source.replace(
                "return replace(initial, **changes)",
                "return replace(initial, **changes, minimum=1)",
            ),
            source.replace(
                "from dataclasses import replace",
                "from unrelated import replace",
            ),
            source.replace("\"initial,target,message\"", "\"target,initial,message\""),
            source.replace("state(mechanisms=2)", "state(**dict(mechanisms=2))"),
            source.replace("state(minimum=128)", "state(minimum=128, minimum=64)"),
            source.replace("\"Tempo\": tempo,", "**dict(Tempo=tempo),"),
            source.replace("\"Tempo\": tempo,", "\"Tempo\": tempo, \"Tempo\": 0,"),
            source.replace(
                "\"LastRateLimitedBlock\": 90",
                "\"LastRateLimitedBlock\": opaque()",
            ),
            source.replace(
                "view = SimpleNamespace(block=100, query=",
                "values = {}\n    view = SimpleNamespace(block=100, query=",
            ),
            source.replace(
                "\"tempo,last,pending,window,frozen\"",
                "\"last,tempo,pending,window,frozen\"",
            ),
            format!("{source}\nstate = None\n"),
            format!("{source}\npytest = None\n"),
            format!("{source}\nREAD_STATE = pruning.read_state\n"),
            format!("{source}\ndef extra(value=opaque()):\n    pass\n"),
        ] {
            ensure!(changed != source);
            ensure!(
                extract(Some(changed)).is_err(),
                "unreviewed pruning producer accepted"
            );
        }
        Ok(())
    }
}
