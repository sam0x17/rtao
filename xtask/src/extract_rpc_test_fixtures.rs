// Derived from RaoFoundation/subtensor sdk/python/tests/unit/test_rpc.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Literal pytest parametrizations projected into the shared named-table corpus.
//!
//! This bounded adapter recognizes the pinned module's inert declaration forms.
//! It reads literal decorator inputs without importing Python or running pytest.

use anyhow::{Result, bail, ensure};
use rustpython_parser::{
    Parse,
    ast::{self, Ranged},
};
use serde_json::{Value, json};

use super::{Bindings, Literal, MAX_CONTAINER_ITEMS, Resolver, call_path, line};

pub(super) const SOURCE: &str = "sdk/python/tests/unit/test_rpc.py";
pub(super) const NAMES: &[&str] = &[
    "test_response_detail_shapes.parametrize",
    "test_provider_defined_code_without_policy_contract_is_not_terminal.parametrize",
];

fn import_aliases(names: &[ast::Alias]) -> String {
    names
        .iter()
        .map(|name| match &name.asname {
            Some(alias) => format!("{}={alias}", name.name),
            None => name.name.to_string(),
        })
        .collect::<Vec<_>>()
        .join(",")
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
        "RPC pytest parametrization function arguments changed"
    );
    Ok(())
}

fn integer_literal(expr: &ast::Expr) -> bool {
    match expr {
        ast::Expr::Constant(value) => matches!(&value.value, ast::Constant::Int(_)),
        ast::Expr::UnaryOp(value) => {
            matches!(value.op, ast::UnaryOp::UAdd | ast::UnaryOp::USub)
                && matches!(&*value.operand, ast::Expr::Constant(value) if matches!(&value.value, ast::Constant::Int(_)))
        }
        _ => false,
    }
}

fn literal_table(
    resolver: &mut Resolver<'_>,
    function: &ast::StmtAsyncFunctionDef,
    source: &str,
    expected: &[&str],
) -> Result<Value> {
    arguments(&function.args, expected)?;
    ensure!(
        function.decorator_list.len() == 1 && function.type_params.is_empty(),
        "RPC pytest parametrization decorators changed"
    );
    let decorator = &function.decorator_list[0];
    let ast::Expr::Call(call) = decorator else {
        bail!("RPC pytest parametrization is not a call")
    };
    ensure!(
        call_path(&call.func).as_deref() == Some("pytest.mark.parametrize")
            && call.args.len() == 2
            && call.keywords.is_empty(),
        "RPC pytest parametrization call changed"
    );
    ensure!(
        matches!(&call.args[0], ast::Expr::Constant(value) if matches!(&value.value, ast::Constant::Str(_)))
            || matches!(&call.args[0], ast::Expr::Tuple(value) if value.elts.iter().all(|value| matches!(value, ast::Expr::Constant(value) if matches!(&value.value, ast::Constant::Str(_))))),
        "RPC pytest parameter names must use literal strings"
    );
    let names = resolver.eval(&call.args[0], &Bindings::new(), SOURCE, 0)?;
    let names = match names {
        Literal::Str(name) => vec![name],
        Literal::Tuple(names) => names
            .into_iter()
            .map(|value| match value {
                Literal::Str(name) => Ok(name),
                _ => bail!("RPC pytest parameter names must be literal strings"),
            })
            .collect::<Result<Vec<_>>>()?,
        _ => bail!("RPC pytest parameter names changed"),
    };
    ensure!(
        names
            .iter()
            .map(String::as_str)
            .eq(expected.iter().copied()),
        "RPC pytest parameter names changed"
    );
    let ast::Expr::List(cases) = &call.args[1] else {
        bail!("RPC pytest parameter cases must be a literal list")
    };
    for case in &cases.elts {
        match expected {
            ["body", "expected"] => ensure!(
                matches!(case, ast::Expr::Tuple(value) if matches!(value.elts.as_slice(), [ast::Expr::Constant(body), ast::Expr::Constant(expected)] if matches!(&body.value, ast::Constant::Bytes(_)) && matches!(&expected.value, ast::Constant::Str(_)))),
                "RPC response-detail cases must use literal bytes/string tuples"
            ),
            ["code"] => ensure!(
                integer_literal(case),
                "RPC provider-code cases must use literal integers"
            ),
            _ => bail!("unreviewed RPC pytest parameter contract"),
        }
    }
    let value = resolver.eval(&call.args[1], &Bindings::new(), SOURCE, 0)?;
    let Literal::List(rows) = &value else {
        bail!("RPC pytest parameter cases must resolve to a list")
    };
    ensure!(
        !rows.is_empty() && rows.len() <= MAX_CONTAINER_ITEMS,
        "RPC pytest parameter case count is empty or unbounded"
    );
    for row in rows {
        match expected {
            ["body", "expected"] => ensure!(
                matches!(row, Literal::Tuple(cells) if matches!(cells.as_slice(), [Literal::Bytes(_), Literal::Str(_)])),
                "RPC response-detail cases must retain bytes/string tuple inputs"
            ),
            ["code"] => ensure!(
                matches!(row, Literal::Int(code) if code.to_string().parse::<i64>().is_ok()),
                "RPC provider-code cases must retain signed i64 inputs"
            ),
            _ => bail!("unreviewed RPC pytest parameter contract"),
        }
    }
    value.allocation_units(0)?;
    Ok(json!({
        "line":line(source, call.args[1].range().start().to_usize()),
        "end_line":line(source, call.args[1].range().end().to_usize()),
        "value":value.json()?,
        "derived":{
            "kind":"pytest.mark.parametrize",
            "function":function.name.as_str(),
            "parameters":names,
            "decorator_line":line(source, decorator.range().start().to_usize()),
            "decorator_end_line":line(source, decorator.range().end().to_usize()),
        }
    }))
}

pub(super) fn values(resolver: &mut Resolver<'_>, names: &[&str]) -> Result<Value> {
    ensure!(names == NAMES, "RPC pytest table selection changed");
    let source = resolver.source(SOURCE)?;
    let suite =
        ast::Suite::parse(&source, SOURCE).map_err(|error| anyhow::anyhow!("{SOURCE}: {error}"))?;
    let mut imports = Vec::new();
    let mut declarations = false;
    let mut pytestmark = false;
    let mut functions = std::collections::BTreeSet::new();
    let mut values = serde_json::Map::new();
    for statement in &suite {
        match statement {
            ast::Stmt::Import(node) => {
                ensure!(
                    !declarations,
                    "RPC pytest imports must precede declarations"
                );
                imports.push(format!("import:{}", import_aliases(&node.names)));
            }
            ast::Stmt::ImportFrom(node) => {
                ensure!(
                    !declarations,
                    "RPC pytest imports must precede declarations"
                );
                imports.push(format!(
                    "from:{}:{}:{}",
                    node.level.map_or(0, |level| level.to_usize()),
                    node.module.as_ref().map_or("", |name| name.as_str()),
                    import_aliases(&node.names)
                ));
            }
            ast::Stmt::Assign(node) => {
                declarations = true;
                ensure!(
                    !pytestmark
                        && node.targets.len() == 1
                        && matches!(&node.targets[0], ast::Expr::Name(name) if name.id.as_str() == "pytestmark")
                        && call_path(&node.value).as_deref() == Some("pytest.mark.asyncio"),
                    "RPC pytest module binding changed"
                );
                pytestmark = true;
            }
            ast::Stmt::AsyncFunctionDef(node) => {
                declarations = true;
                ensure!(
                    functions.insert(node.name.as_str())
                        && node.name.as_str().starts_with("test_")
                        && node.type_params.is_empty(),
                    "RPC pytest function binding changed"
                );
                let expected: &[&str] = match node.name.as_str() {
                    "test_response_detail_shapes" => &["body", "expected"],
                    "test_provider_defined_code_without_policy_contract_is_not_terminal" => {
                        &["code"]
                    }
                    _ => &[],
                };
                if expected.is_empty() {
                    ensure!(
                        node.decorator_list.is_empty()
                            && node
                                .args
                                .posonlyargs
                                .iter()
                                .chain(&node.args.args)
                                .chain(&node.args.kwonlyargs)
                                .all(|arg| arg.default.is_none()),
                        "RPC pytest added executable decorator/default requires review"
                    );
                } else {
                    let key = format!("{}.parametrize", node.name);
                    ensure!(
                        values
                            .insert(key, literal_table(resolver, node, &source, expected)?)
                            .is_none(),
                        "duplicate RPC pytest parameter table"
                    );
                }
            }
            ast::Stmt::FunctionDef(node) => {
                declarations = true;
                ensure!(
                    functions.insert(node.name.as_str())
                        && (node.name.as_str() == "make_session"
                            || node.name.as_str().starts_with("test_"))
                        && node.decorator_list.is_empty()
                        && node.type_params.is_empty()
                        && node
                            .args
                            .posonlyargs
                            .iter()
                            .chain(&node.args.args)
                            .chain(&node.args.kwonlyargs)
                            .all(|arg| arg.default.is_none()),
                    "RPC pytest executable declaration requires review"
                );
            }
            ast::Stmt::Expr(node) if matches!(&*node.value, ast::Expr::Constant(node) if matches!(&node.value, ast::Constant::Str(_))) =>
                {}
            _ => bail!("RPC pytest executable module statement requires review"),
        }
    }
    ensure!(imports.iter().map(String::as_str).eq([
        "from:0:__future__:annotations", "import:asyncio", "import:pytest",
        "from:0:websockets.datastructures:Headers",
        "from:0:websockets.exceptions:ConnectionClosedError,InvalidStatus",
        "from:0:websockets.frames:Close", "from:0:websockets.http11:Response",
        "from:0:bittensor:RpcConnectionError",
        "from:0:bittensor:RpcPolicyError=PublicRpcPolicyError",
        "from:0:bittensor._substrate:RpcSubstrate",
        "from:0:bittensor._transport:rpc",
        "from:0:bittensor._transport.errors:MaxRetriesExceeded,RpcPolicyRejection,StateDiscardedError,SubstrateRequestException",
        "from:0:bittensor._transport.rpc:RpcSession",
        "from:0:tests.harness.fake_node:FakeNode",
    ]), "RPC pytest import producers changed");
    ensure!(
        pytestmark
            && values.len() == NAMES.len()
            && NAMES.iter().all(|name| values.contains_key(*name)),
        "RPC pytest parameter table inventory changed"
    );
    Ok(Value::Object(values))
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Context;
    use std::path::PathBuf;

    fn root() -> Result<PathBuf> {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .context("workspace root")
            .map(PathBuf::from)
    }
    fn extract(source: Option<String>) -> Result<Value> {
        let root = root()?;
        let pins = std::fs::read_to_string(root.join("upstream.toml"))?.parse()?;
        let mut resolver = Resolver::new(&root)?;
        if let Some(source) = source {
            resolver.sources.insert(SOURCE.into(), source);
        }
        super::super::document(&mut resolver, &pins, SOURCE, NAMES)
    }

    #[test]
    fn rpc_pytest_inputs_preserve_literal_carriers_and_authenticated_spans() -> Result<()> {
        let actual = extract(None)?;
        let root = root()?;
        let resolver = Resolver::new(&root)?;
        let source = resolver.source(SOURCE)?;
        ensure!(
            actual["provenance"]["path"] == SOURCE
                && actual["provenance"]["blob"] == resolver.tree[SOURCE]
        );
        let details = &actual["values"][NAMES[0]];
        let codes = &actual["values"][NAMES[1]];
        ensure!(details["value"].as_array().context("detail rows")?.len() == 5);
        ensure!(codes["value"].as_array().context("code rows")?.len() == 2);
        for (name, row) in actual["values"].as_object().context("values")? {
            ensure!(row["line"].as_u64().context("start line")? > 0);
            ensure!(
                row["end_line"].as_u64().context("end line")?
                    >= row["line"].as_u64().context("start")?
            );
            ensure!(row["derived"]["kind"] == "pytest.mark.parametrize");
            ensure!(
                name == &format!(
                    "{}.parametrize",
                    row["derived"]["function"].as_str().context("function")?
                )
            );
        }
        // A legitimate literal update must change the produced row, without
        // editing an expected Rust table or routing bytes through UTF-8.
        let changed = source.replace(
            "b\"temporarily unavailable\", \"temporarily unavailable\"",
            "b\"\\xff\", \"updated literal\"",
        );
        ensure!(changed != source);
        let updated = extract(Some(changed))?;
        ensure!(updated["values"][NAMES[0]]["value"][1]["tuple"][0]["bytes_hex"] == "ff");
        ensure!(updated["values"][NAMES[0]]["value"][1]["tuple"][1] == "updated literal");
        ensure!(&updated["values"][NAMES[1]] == codes);
        Ok(())
    }

    #[test]
    fn rpc_pytest_input_updates_and_spans_follow_the_actual_decorator() -> Result<()> {
        let root = root()?;
        let resolver = Resolver::new(&root)?;
        let source = resolver.source(SOURCE)?;
        let original = extract(None)?;
        let changed = source.replace("[-32004, -32005]", "[-32123]");
        ensure!(changed != source);
        let updated = extract(Some(changed))?;
        ensure!(updated["values"][NAMES[1]]["value"] == json!([-32123]));
        ensure!(updated["values"][NAMES[0]] == original["values"][NAMES[0]]);
        let shifted = extract(Some(format!(
            "# unchanged source forms after a line shift\n{source}"
        )))?;
        for name in NAMES {
            ensure!(shifted["values"][*name]["value"] == original["values"][*name]["value"]);
            for field in ["line", "end_line"] {
                ensure!(
                    shifted["values"][*name][field].as_u64()
                        == original["values"][*name][field]
                            .as_u64()
                            .map(|line| line + 1)
                );
            }
            for field in ["decorator_line", "decorator_end_line"] {
                ensure!(
                    shifted["values"][*name]["derived"][field].as_u64()
                        == original["values"][*name]["derived"][field]
                            .as_u64()
                            .map(|line| line + 1)
                );
            }
        }
        Ok(())
    }

    #[test]
    fn rpc_pytest_producer_refuses_changed_bindings_shape_and_executable_defaults() -> Result<()> {
        let root = root()?;
        let resolver = Resolver::new(&root)?;
        let source = resolver.source(SOURCE)?;
        for changed in [
            source.replace("import pytest", "import unrelated as pytest"),
            source.replace("pytestmark = pytest.mark.asyncio", "pytest = None"),
            source.replace("(\"body\", \"expected\")", "(\"expected\", \"body\")"),
            source.replace(
                "@pytest.mark.parametrize(\"code\",",
                "@pytest.mark.parametrize(\"codes\",",
            ),
            source.replace("[-32004, -32005]", "[True]"),
            source.replace("[-32004, -32005]", "[-32004 + 0, -32005]"),
            source.replace("[-32004, -32005]", "[int('-32004'), -32005]"),
            source.replace("[-32004, -32005]", "[-9223372036854775809]"),
            source.replace("[-32004, -32005]", "[]"),
            source.replace("[-32004, -32005]", "opaque()"),
            source.replace("(b\"\", \"\")", "(\"\", \"\")"),
            source.replace(
                "async def test_response_detail_shapes(body, expected):",
                "async def test_response_detail_shapes(body, expected=opaque()):",
            ),
            source.replace(
                "async def test_response_detail_shapes(body, expected):",
                "def test_response_detail_shapes(body, expected):",
            ),
            format!("{source}\npytest.mark.parametrize = None\n"),
            format!("{source}\nopaque(pytest)\n"),
            format!("{source}\ndef pytest_generate_tests(metafunc):\n    pass\n"),
            format!("{source}\ndef __getattr__(name):\n    pass\n"),
            format!("{source}\ndef test_new_default(value=opaque()):\n    pass\n"),
            format!(
                "{source}\n@pytest.mark.parametrize('code', [1])\nasync def test_new_parameter_table(code):\n    pass\n"
            ),
            format!("{source}\nasync def test_response_detail_shapes(body, expected):\n    pass\n"),
        ] {
            ensure!(changed != source);
            ensure!(
                extract(Some(changed)).is_err(),
                "unsupported RPC fixture producer was accepted"
            );
        }
        Ok(())
    }
}
