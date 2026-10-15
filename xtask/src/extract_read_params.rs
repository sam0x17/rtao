//! Read declarations and CLI requiredness from authenticated Python sources.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use anyhow::{Context, Result, bail, ensure};
use rustpython_parser::{
    Mode, Parse, Tok,
    ast::{self, Ranged},
    lexer,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use toml_edit::DocumentMut;

use super::{Bindings, Literal, Resolver, call_path, line, pretty};

const READS: &str = "sdk/python/bittensor/reads/";
const BASE: &str = "sdk/python/bittensor/reads/base.py";
const INIT: &str = "sdk/python/bittensor/reads/__init__.py";
const QUERY: &str = "sdk/python/bittensor/cli/query.py";
const HYPER: &str = "sdk/python/bittensor/reads/hyperparameters.py";
const CATALOG: &str = "vendor/upstream/catalog/reads.json";
const OUTPUT: &str = "vendor/upstream/extracted/read_params.json";
const READ_COUNT: usize = 102;

struct Source {
    path: String,
    blob: String,
    text: String,
    suite: ast::Suite,
}

impl Source {
    fn load(resolver: &Resolver<'_>, path: &str) -> Result<Self> {
        let text = resolver.source(path)?;
        let suite =
            ast::Suite::parse(&text, path).map_err(|error| anyhow::anyhow!("{path}: {error}"))?;
        ensure!(
            suite.len() <= super::MAX_CONTAINER_ITEMS,
            "{path}: read module exceeds statement limit"
        );
        Ok(Self {
            path: path.into(),
            blob: resolver
                .tree
                .get(path)
                .context("source blob missing")?
                .clone(),
            text,
            suite,
        })
    }

    fn span(&self, node: &impl Ranged) -> Value {
        self.byte_span(
            node.range().start().to_usize(),
            node.range().end().to_usize(),
        )
    }

    fn byte_span(&self, start: usize, end: usize) -> Value {
        json!({"path":self.path,"blob":self.blob,"line":line(&self.text,start),"end_line":line(&self.text,end),"start_byte":start,"end_byte":end})
    }

    fn text(&self, node: &impl Ranged) -> &str {
        &self.text[node.range().start().to_usize()..node.range().end().to_usize()]
    }

    fn function(&self, name: &str) -> Result<&ast::StmtFunctionDef> {
        let declarations = self
            .suite
            .iter()
            .filter_map(|stmt| match stmt {
                ast::Stmt::FunctionDef(function) if function.name.as_str() == name => {
                    Some(function)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        ensure!(
            declarations.len() == 1,
            "{}: expected one {name} declaration",
            self.path
        );
        self.unique_binding(name)?;
        ensure!(
            declarations[0].decorator_list.is_empty(),
            "{}: {name} must remain undecorated",
            self.path
        );
        Ok(declarations[0])
    }

    fn unique_binding(&self, name: &str) -> Result<()> {
        let count = self
            .suite
            .iter()
            .filter(|stmt| {
                super::super::effects::conditional_names(std::slice::from_ref(*stmt)).contains(name)
            })
            .count();
        ensure!(count == 1, "{}: {name} is absent or rebound", self.path);
        Ok(())
    }

    fn standard_import(&self, name: &str) -> Result<()> {
        self.unique_binding(name)?;
        ensure!(
            self.suite.iter().any(|stmt| {
                matches!(stmt, ast::Stmt::Import(import)
                    if import.names.iter().any(|alias| alias.name.as_str() == name && alias.asname.is_none()))
            }),
            "{}: {name} must refer to the standard-library module",
            self.path
        );
        Ok(())
    }

    /// Compare executable tokens, including indentation, without comments or
    /// spelling differences between equivalent string/number literals.
    fn expect(&self, stmt: &ast::Stmt, expected: &str) -> Result<()> {
        let start = stmt.range().start().to_usize();
        let line_start = self.text[..start].rfind('\n').map_or(0, |index| index + 1);
        let column = start - line_start;
        let mut text = String::new();
        for (index, line) in self.text(stmt).lines().enumerate() {
            if index > 0 {
                text.push('\n');
                let leading = line.bytes().take_while(|byte| *byte == b' ').count();
                text.push_str(&line[column.min(leading)..]);
            } else {
                text.push_str(line);
            }
        }
        ensure!(
            tokens(&text)? == tokens(expected)?,
            "{}:{}: read parameter rule changed; requires a hand port",
            self.path,
            line(&self.text, start)
        );
        Ok(())
    }
}

fn tokens(source: &str) -> Result<Vec<Tok>> {
    lexer::lex(source, Mode::Module)
        .map(|token| {
            token
                .map(|(token, _)| token)
                .map_err(|error| anyhow::anyhow!("{error:?}"))
        })
        .collect()
}

fn body(statements: &[ast::Stmt]) -> &[ast::Stmt] {
    if matches!(statements.first(), Some(ast::Stmt::Expr(expr)) if matches!(&*expr.value,ast::Expr::Constant(value) if matches!(value.value,ast::Constant::Str(_))))
    {
        &statements[1..]
    } else {
        statements
    }
}

fn signature(args: &ast::Arguments, positional: &[&str], keyword: &[&str]) -> Result<()> {
    ensure!(
        args.posonlyargs.is_empty() && args.vararg.is_none() && args.kwarg.is_none(),
        "read helper signature changed"
    );
    ensure!(
        args.args
            .iter()
            .map(|arg| arg.def.arg.as_str())
            .eq(positional.iter().copied())
            && args
                .kwonlyargs
                .iter()
                .map(|arg| arg.def.arg.as_str())
                .eq(keyword.iter().copied()),
        "read helper parameter list changed"
    );
    Ok(())
}

fn requiredness(resolver: &mut Resolver<'_>) -> Result<(Value, BTreeSet<String>)> {
    let source = Source::load(resolver, QUERY)?;
    source.standard_import("inspect")?;
    source.unique_binding("_OWN_KEY")?;
    let own = source.suite.iter().find_map(|stmt| match stmt {
        ast::Stmt::Assign(assign) if assign.targets.len()==1 && matches!(&assign.targets[0],ast::Expr::Name(name) if name.id.as_str()=="_OWN_KEY") => Some((stmt,&*assign.value)),
        _ => None,
    }).context("query _OWN_KEY assignment missing")?;
    let Literal::Tuple(keys) = resolver.eval(own.1, &Bindings::new(), QUERY, 0)? else {
        bail!("query _OWN_KEY must be a literal tuple");
    };
    let keys = keys
        .into_iter()
        .map(|value| match value {
            Literal::Str(name) => Ok(name),
            _ => bail!("query _OWN_KEY must contain strings"),
        })
        .collect::<Result<Vec<_>>>()?;
    ensure!(
        keys == ["hotkey_ss58", "coldkey_ss58"],
        "query wallet-key exceptions changed"
    );
    let default = source.function("_fetch_default")?;
    signature(&default.args, &["pname", "fetch_params"], &[])?;
    ensure!(
        default.args.args.iter().all(|arg| arg.default.is_none()),
        "_fetch_default gained argument defaults"
    );
    let default_body = body(&default.body);
    ensure!(default_body.len() == 2, "query _fetch_default body changed");
    source.expect(
        &default_body[0],
        "if pname not in fetch_params:\n    return inspect.Parameter.empty",
    )?;
    source.expect(&default_body[1], "return fetch_params[pname].default")?;
    let required = source.function("_is_required")?;
    signature(&required.args, &["pname", "fetch_params"], &[])?;
    ensure!(
        required.args.args.iter().all(|arg| arg.default.is_none()),
        "_is_required gained argument defaults"
    );
    let required_body = body(&required.body);
    ensure!(required_body.len() == 3, "query _is_required body changed");
    source.expect(&required_body[0], "if pname in _OWN_KEY:\n    return False")?;
    source.expect(
        &required_body[1],
        "default = _fetch_default(pname, fetch_params)",
    )?;
    source.expect(
        &required_body[2],
        "return default is inspect.Parameter.empty or default is None",
    )?;
    Ok((
        json!({"own_key":keys,"missing_or_none_required":true,"provenance":{"own_key":source.span(own.0),"fetch_default":source.span(default),"is_required":source.span(required)}}),
        keys.into_iter().collect(),
    ))
}

fn dictionary(
    resolver: &mut Resolver<'_>,
    expr: &ast::Expr,
    path: &str,
) -> Result<Vec<(String, String)>> {
    let Literal::Dict(entries) = resolver.eval(expr, &Bindings::new(), path, 0)? else {
        bail!("{path}: read params must be a resolved literal dictionary");
    };
    entries
        .into_iter()
        .map(|(key, value)| match (key, value) {
            (Literal::Str(key), Literal::Str(value)) => {
                ensure!(
                    ["string", "integer", "number", "boolean", "array"].contains(&value.as_str()),
                    "{path}: unsupported read JSON type {value}"
                );
                Ok((key, value))
            }
            _ => bail!("{path}: read params must map strings to JSON type strings"),
        })
        .collect()
}

fn literal_string(resolver: &mut Resolver<'_>, expr: &ast::Expr, path: &str) -> Result<String> {
    match resolver.eval(expr, &Bindings::new(), path, 0)? {
        Literal::Str(value) => Ok(value),
        _ => bail!("{path}: read name/category must be a resolved string"),
    }
}

fn keyword<'a>(call: &'a ast::ExprCall, name: &str) -> Result<&'a ast::Expr> {
    let found = call
        .keywords
        .iter()
        .filter(|kw| kw.arg.as_ref().is_some_and(|arg| arg.as_str() == name))
        .collect::<Vec<_>>();
    ensure!(
        found.len() == 1,
        "read registration requires exactly one {name} keyword"
    );
    Ok(&found[0].value)
}

fn keywords(call: &ast::ExprCall, allowed: &[&str]) -> Result<()> {
    let mut names = BTreeSet::new();
    for kw in &call.keywords {
        let name = kw
            .arg
            .as_ref()
            .context("read registration cannot unpack keyword arguments")?
            .as_str();
        ensure!(
            allowed.contains(&name) && names.insert(name),
            "unsupported or repeated read keyword {name}"
        );
    }
    Ok(())
}

fn parameters(
    resolver: &mut Resolver<'_>,
    source: &Source,
    args: &ast::Arguments,
    declared: &[(String, String)],
    own_key: &BTreeSet<String>,
    scalar: bool,
) -> Result<Vec<Value>> {
    ensure!(
        args.posonlyargs.is_empty() && args.vararg.is_none() && args.kwarg.is_none(),
        "{}: read fetch cannot hide positional-only or variadic params",
        source.path
    );
    ensure!(
        matches!(args.args.first(),Some(arg) if arg.def.arg.as_str()=="view" && arg.default.is_none()),
        "{}: read fetch must start with view",
        source.path
    );
    let arguments = args.args[1..]
        .iter()
        .chain(&args.kwonlyargs)
        .map(|arg| (arg.def.arg.as_str(), arg))
        .collect::<BTreeMap<_, _>>();
    let declared_names = declared
        .iter()
        .map(|(name, _)| name.as_str())
        .collect::<BTreeSet<_>>();
    ensure!(
        arguments.len() == args.args.len() - 1 + args.kwonlyargs.len(),
        "{}: duplicate fetch param",
        source.path
    );
    ensure!(
        arguments.keys().copied().collect::<BTreeSet<_>>() == declared_names
            || (scalar
                && declared.is_empty()
                && arguments.len() == 1
                && arguments.contains_key("netuid")),
        "{}: declared params and fetch signature differ",
        source.path
    );
    let mut out = Vec::new();
    for (name, json_type) in declared {
        let arg = arguments
            .get(name.as_str())
            .context("declared read param missing from fetch")?;
        let default = arg
            .default
            .as_deref()
            .map(|expr| {
                resolver
                    .eval(expr, &Bindings::new(), &source.path, 0)
                    .and_then(|value| value.json())
            })
            .transpose()
            .with_context(|| format!("{}: default for {name} requires a hand port", source.path))?;
        let is_own = own_key.contains(name);
        let required = !is_own && default.as_ref().is_none_or(Value::is_null);
        let start = arg.def.range.start().to_usize();
        let end = arg
            .default
            .as_ref()
            .map_or(arg.def.range.end().to_usize(), |expr| {
                expr.range().end().to_usize()
            });
        out.push(json!({"name":name,"json_type":json_type,"annotation":arg.def.annotation.as_ref().map(|annotation|source.text(&**annotation)),"default":default.map_or_else(||json!({"present":false}),|value|json!({"present":true,"value":value})),"required":required,"own_key":is_own,"provenance":source.byte_span(start,end)}));
    }
    Ok(out)
}

fn scalar_factory(source: &Source) -> Result<&ast::StmtAsyncFunctionDef> {
    let function = source.function("scalar_read")?;
    signature(
        &function.args,
        &["name", "item"],
        &["per_netuid", "doc", "category"],
    )?;
    ensure!(
        function
            .args
            .args
            .iter()
            .chain(&function.args.kwonlyargs)
            .all(|arg| arg.default.is_none()),
        "scalar_read factory defaults changed"
    );
    let statements = body(&function.body);
    ensure!(statements.len() == 5, "scalar_read factory body changed");
    source.expect(
        &statements[0],
        "params = {\"netuid\": \"integer\"} if per_netuid else {}",
    )?;
    source.expect(
        &statements[1],
        "param_docs = {\"netuid\": \"Subnet to query.\"} if per_netuid else {}",
    )?;
    let ast::Stmt::AsyncFunctionDef(fetch) = &statements[2] else {
        bail!("scalar_read fetch function missing");
    };
    ensure!(
        fetch.name.as_str() == "fetch" && fetch.decorator_list.is_empty(),
        "scalar_read fetch wrapping changed"
    );
    signature(&fetch.args, &["view", "netuid"], &[])?;
    ensure!(
        fetch.args.args[0].default.is_none()
            && matches!(fetch.args.args[1].default.as_deref(),Some(ast::Expr::Constant(value)) if value.value==ast::Constant::None),
        "scalar_read fetch defaults changed"
    );
    let fetch_body = body(&fetch.body);
    ensure!(fetch_body.len() == 2, "scalar_read fetch body changed");
    source.expect(&fetch_body[0], "keys = [netuid] if per_netuid else None")?;
    source.expect(&fetch_body[1], "return int(await view.query(item, keys))")?;
    source.expect(&statements[3], "fetch.__doc__ = doc")?;
    source.expect(
        &statements[4],
        "read(name, params, category=category, param_docs=param_docs)(fetch)",
    )?;
    Ok(fetch)
}

fn registration_rule(source: &Source) -> Result<Value> {
    source.standard_import("inspect")?;
    source.standard_import("functools")?;
    let read = source.function("read")?;
    signature(
        &read.args,
        &["name", "params"],
        &["category", "param_docs", "render"],
    )?;
    let statements = body(&read.body);
    ensure!(statements.len() == 2, "read decorator body changed");
    let ast::Stmt::FunctionDef(decorate) = &statements[0] else {
        bail!("read decorate function missing");
    };
    signature(&decorate.args, &["fn"], &[])?;
    ensure!(
        decorate.name.as_str() == "decorate" && decorate.decorator_list.is_empty(),
        "read decorate wrapper changed"
    );
    let statements = body(&decorate.body);
    ensure!(statements.len() == 7, "read registration body changed");
    for (stmt, expected) in statements.iter().zip([
        "if name in REGISTRY:\n    raise ValueError(f\"Duplicate read: {name}\")",
        "doc = inspect.cleandoc(fn.__doc__ or \"\").replace(\"``\", \"`\")",
        "unknown = set(param_docs or {}) - set(params or {})",
        "if unknown:\n    raise ValueError(f\"param_docs for unknown params of read {name!r}: {sorted(unknown)}\")",
        "registered = _coercing(fn, params or {})",
        "REGISTRY[name] = ReadSpec(name, doc, params or {}, registered, category, param_docs or {}, render)",
        "return registered",
    ]) { source.expect(stmt,expected)?; }
    source.expect(&body(&read.body)[1], "return decorate")?;
    let coercing = source.function("_coercing")?;
    signature(&coercing.args, &["fn", "params"], &[])?;
    let statements = body(&coercing.body);
    ensure!(statements.len() == 5, "read coercing body changed");
    source.expect(
        &statements[0],
        "ss58_params = [p for p in params if is_address_param(p)]",
    )?;
    source.expect(&statements[1], "if not ss58_params:\n    return fn")?;
    source.expect(&statements[2], "sig = inspect.signature(fn)")?;
    let ast::Stmt::AsyncFunctionDef(wrapped) = &statements[3] else {
        bail!("read coercing wrapper missing");
    };
    ensure!(
        wrapped.name.as_str() == "wrapped"
            && wrapped.decorator_list.len() == 1
            && matches!(&wrapped.decorator_list[0],ast::Expr::Call(call) if call_path(&call.func).as_deref()==Some("functools.wraps") && call.args.len()==1 && matches!(&call.args[0],ast::Expr::Name(name) if name.id.as_str()=="fn") && call.keywords.is_empty()),
        "read coercing wrapper no longer preserves fetch signature"
    );
    source.expect(&statements[4], "return wrapped")?;
    Ok(
        json!({"read":source.span(read),"decorate":source.span(decorate),"signature_wrapper":source.span(wrapped)}),
    )
}

fn module_imports(source: &Source) -> Result<BTreeSet<String>> {
    let mut imports = BTreeSet::new();
    for stmt in &source.suite {
        if let ast::Stmt::ImportFrom(import) = stmt
            && import
                .module
                .as_ref()
                .is_some_and(|module| module.as_str() == "base")
            && import.level.is_some_and(|level| level.to_usize() == 1)
        {
            for alias in &import.names {
                if matches!(alias.name.as_str(), "read" | "scalar_read") {
                    ensure!(
                        alias.asname.is_none() && imports.insert(alias.name.to_string()),
                        "{}: read factory alias requires review",
                        source.path
                    );
                    source.unique_binding(alias.name.as_str())?;
                }
            }
        }
    }
    Ok(imports)
}

fn module_paths(resolver: &Resolver<'_>) -> Result<(Vec<String>, Value)> {
    let source = Source::load(resolver, INIT)?;
    let mut registered = BTreeSet::new();
    let mut declarations = Vec::new();
    for stmt in &source.suite {
        ensure!(
            match stmt {
                ast::Stmt::ImportFrom(_) => true,
                ast::Stmt::Expr(expr) =>
                    matches!(&*expr.value,ast::Expr::Constant(value) if matches!(value.value,ast::Constant::Str(_))),
                ast::Stmt::Assign(assign) =>
                    assign.targets.len() == 1
                        && matches!(&assign.targets[0],ast::Expr::Name(name) if name.id.as_str()=="__all__")
                        && matches!(&*assign.value,ast::Expr::List(list) if list.elts.iter().all(|expr|matches!(expr,ast::Expr::Constant(value) if matches!(value.value,ast::Constant::Str(_))))),
                _ => false,
            },
            "read package initialization has dynamic registration or mutation"
        );
        if let ast::Stmt::ImportFrom(import) = stmt
            && import.module.is_none()
            && import.level.is_some_and(|level| level.to_usize() == 1)
        {
            for alias in &import.names {
                ensure!(
                    alias.asname.is_none(),
                    "read registration module alias requires review"
                );
                source.unique_binding(alias.name.as_str())?;
                ensure!(
                    registered.insert(format!("{READS}{}.py", alias.name)),
                    "duplicate registration module"
                );
            }
            declarations.push(source.span(stmt));
        }
    }
    let available = resolver
        .tree
        .keys()
        .filter(|path| {
            path.starts_with(READS)
                && path.ends_with(".py")
                && path.as_str() != BASE
                && path.as_str() != INIT
        })
        .cloned()
        .collect::<BTreeSet<_>>();
    ensure!(
        registered == available,
        "reads package imports and authenticated read modules differ"
    );
    Ok((
        registered.into_iter().collect(),
        json!({"imports":declarations}),
    ))
}

fn insert_read(reads: &mut BTreeMap<String, Value>, read: Value) -> Result<()> {
    let name = read["name"]
        .as_str()
        .context("read name missing")?
        .to_owned();
    ensure!(
        reads.insert(name.clone(), read).is_none(),
        "duplicate read registration {name}"
    );
    ensure!(
        reads.len() <= super::MAX_CONTAINER_ITEMS,
        "read registry exceeds item limit"
    );
    Ok(())
}

fn factory_uses(statements: &[ast::Stmt]) -> Result<Vec<(usize, usize)>> {
    use rustpython_ast::Visitor;

    #[derive(Default)]
    struct Uses {
        names: Vec<(usize, usize)>,
        depth: usize,
        exceeded: bool,
    }
    impl Visitor for Uses {
        fn visit_stmt(&mut self, node: ast::Stmt) {
            if self.depth >= super::super::MAX_DEPTH {
                self.exceeded = true;
                return;
            }
            self.depth += 1;
            self.generic_visit_stmt(node);
            self.depth -= 1;
        }

        fn visit_expr(&mut self, node: ast::Expr) {
            if self.depth >= super::super::MAX_DEPTH {
                self.exceeded = true;
                return;
            }
            self.depth += 1;
            self.generic_visit_expr(node);
            self.depth -= 1;
        }

        fn visit_expr_name(&mut self, node: ast::ExprName) {
            if node.id.as_str() == "REGISTRY"
                || (node.ctx == ast::ExprContext::Load
                    && matches!(node.id.as_str(), "read" | "scalar_read" | "_hyperparam"))
            {
                if self.names.len() >= super::super::MAX_CONTAINER_ITEMS {
                    self.exceeded = true;
                    return;
                }
                self.names
                    .push((node.range.start().to_usize(), node.range.end().to_usize()));
            }
        }
    }
    let mut uses = Uses::default();
    for stmt in statements {
        uses.visit_stmt(stmt.clone());
    }
    ensure!(
        !uses.exceeded,
        "read factory scan exceeds extraction bounds"
    );
    Ok(uses.names)
}

fn extract_module(
    resolver: &mut Resolver<'_>,
    source: &Source,
    base: &Source,
    fetch: &ast::StmtAsyncFunctionDef,
    own_key: &BTreeSet<String>,
    reads: &mut BTreeMap<String, Value>,
) -> Result<()> {
    let imports = module_imports(source)?;
    let mut accepted_factory_names = BTreeSet::new();
    let hyper = if source.path == HYPER {
        let helper = source.function("_hyperparam")?;
        signature(&helper.args, &["name", "item", "doc"], &[])?;
        ensure!(
            helper.args.args.iter().all(|arg| arg.default.is_none()),
            "_hyperparam helper defaults changed"
        );
        let statements = body(&helper.body);
        ensure!(statements.len() == 1, "_hyperparam helper body changed");
        source.expect(
            &statements[0],
            "scalar_read(name, item, per_netuid=True, doc=doc, category=\"Hyperparameters\")",
        )?;
        let ast::Stmt::Expr(expr) = &statements[0] else {
            bail!("_hyperparam body must call scalar_read");
        };
        let ast::Expr::Call(call) = &*expr.value else {
            bail!("_hyperparam body must call scalar_read");
        };
        accepted_factory_names.insert((
            call.func.range().start().to_usize(),
            call.func.range().end().to_usize(),
        ));
        ensure!(
            imports.contains("scalar_read"),
            "_hyperparam scalar factory import missing"
        );
        Some(helper)
    } else {
        None
    };
    for stmt in &source.suite {
        match stmt {
            ast::Stmt::AsyncFunctionDef(function) if !function.decorator_list.is_empty() => {
                ensure!(
                    function.decorator_list.len() == 1 && imports.contains("read"),
                    "{}: decorated read form changed",
                    source.path
                );
                let ast::Expr::Call(call) = &function.decorator_list[0] else {
                    bail!("read decorator must be a call");
                };
                ensure!(
                    call_path(&call.func).as_deref() == Some("read") && call.args.len() == 2,
                    "read decorator signature changed"
                );
                accepted_factory_names.insert((
                    call.func.range().start().to_usize(),
                    call.func.range().end().to_usize(),
                ));
                keywords(call, &["category", "param_docs", "render"])?;
                let name = literal_string(resolver, &call.args[0], &source.path)?;
                let declared = dictionary(resolver, &call.args[1], &source.path)?;
                let category = literal_string(resolver, keyword(call, "category")?, &source.path)?;
                let params =
                    parameters(resolver, source, &function.args, &declared, own_key, false)?;
                insert_read(
                    reads,
                    json!({"name":name,"category":category,"params":params,"provenance":{"declaration":source.span(call),"fetch":source.span(function)}}),
                )?;
            }
            ast::Stmt::Expr(expr) => {
                if matches!(&*expr.value,ast::Expr::Constant(value) if matches!(value.value,ast::Constant::Str(_)))
                {
                    continue;
                }
                let ast::Expr::Call(call) = &*expr.value else {
                    bail!("{}: unsupported read module expression", source.path);
                };
                let callee =
                    call_path(&call.func).context("read registration callee must be a name")?;
                ensure!(
                    (callee == "scalar_read" && imports.contains("scalar_read"))
                        || (callee == "_hyperparam" && hyper.is_some()),
                    "{}: unknown module-level read registration {callee}",
                    source.path
                );
                accepted_factory_names.insert((
                    call.func.range().start().to_usize(),
                    call.func.range().end().to_usize(),
                ));
                let name = literal_string(
                    resolver,
                    call.args.first().context("scalar read name missing")?,
                    &source.path,
                )?;
                let (per_netuid, category) = if callee == "_hyperparam" {
                    ensure!(
                        call.args.len() == 3 && call.keywords.is_empty(),
                        "_hyperparam registration signature changed"
                    );
                    (true, "Hyperparameters".to_owned())
                } else {
                    ensure!(
                        call.args.len() == 2,
                        "scalar_read registration signature changed"
                    );
                    keywords(call, &["per_netuid", "doc", "category"])?;
                    let Literal::Bool(per_netuid) = resolver.eval(
                        keyword(call, "per_netuid")?,
                        &Bindings::new(),
                        &source.path,
                        0,
                    )?
                    else {
                        bail!("scalar_read per_netuid must be a resolved bool");
                    };
                    keyword(call, "doc")?;
                    (
                        per_netuid,
                        literal_string(resolver, keyword(call, "category")?, &source.path)?,
                    )
                };
                let declared = if per_netuid {
                    vec![("netuid".into(), "integer".into())]
                } else {
                    vec![]
                };
                let params = parameters(resolver, base, &fetch.args, &declared, own_key, true)?;
                let mut provenance = json!({"declaration":source.span(call),"fetch":base.span(fetch),"registration_rule":base.span(base.function("scalar_read")?)});
                if let Some(helper) = hyper {
                    provenance["helper"] = source.span(helper);
                }
                insert_read(
                    reads,
                    json!({"name":name,"category":category,"params":params,"provenance":provenance}),
                )?;
            }
            ast::Stmt::Import(_)
            | ast::Stmt::ImportFrom(_)
            | ast::Stmt::FunctionDef(_)
            | ast::Stmt::AsyncFunctionDef(_)
            | ast::Stmt::ClassDef(_)
            | ast::Stmt::Assign(_)
            | ast::Stmt::AnnAssign(_) => {}
            _ => bail!(
                "{}:{}: dynamic read registration requires a hand port",
                source.path,
                line(&source.text, stmt.range().start().to_usize())
            ),
        }
    }
    let unsupported = factory_uses(&source.suite)?
        .into_iter()
        .find(|span| !accepted_factory_names.contains(span));
    ensure!(
        unsupported.is_none(),
        "{}:{}: read factory alias or hidden registration requires a hand port",
        source.path,
        line(&source.text, unsupported.map_or(0, |span| span.0))
    );
    Ok(())
}

fn catalog(root: &Path, pins: &DocumentMut) -> Result<(Value, Value)> {
    let (path, row) = pins["artifacts"]
        .as_table_like()
        .context("artifact pins missing")?
        .iter()
        .find(|(_, row)| row.get("to").and_then(|item| item.as_str()) == Some(CATALOG))
        .context("read catalog artifact pin missing")?;
    let expected = row
        .get("sha256")
        .and_then(|item| item.as_str())
        .context("read catalog hash missing")?;
    let bytes = fs::read(root.join(CATALOG))?;
    ensure!(
        bytes.len() <= super::MAX_SOURCE_BYTES,
        "read catalog exceeds byte limit"
    );
    ensure!(
        format!("{:x}", Sha256::digest(&bytes)) == expected,
        "{CATALOG}: catalog differs from artifact pin"
    );
    Ok((
        serde_json::from_slice(&bytes)?,
        json!({"path":path,"sha256":expected,"tag":pins["parity"]["tag"].as_str(),"commit":pins["parity"]["commit"].as_str(),"count":READ_COUNT}),
    ))
}

fn compare_catalog(reads: &BTreeMap<String, Value>, catalog: &Value) -> Result<()> {
    let rows = catalog
        .as_array()
        .context("read catalog must be an array")?;
    ensure!(
        rows.len() == READ_COUNT && reads.len() == READ_COUNT,
        "source and catalog must each declare {READ_COUNT} reads"
    );
    let mut names = BTreeSet::new();
    for row in rows {
        let name = row["name"].as_str().context("catalog read name missing")?;
        ensure!(names.insert(name), "duplicate catalog read {name}");
        let read = reads
            .get(name)
            .with_context(|| format!("catalog read {name} absent from source"))?;
        ensure!(
            read["category"] == row["category"],
            "read {name}: source/category mismatch"
        );
        let params = read["params"]
            .as_array()
            .context("source read params missing")?
            .iter()
            .map(|param| {
                Ok((
                    param["name"]
                        .as_str()
                        .context("param name missing")?
                        .to_owned(),
                    param["json_type"].clone(),
                ))
            })
            .collect::<Result<serde_json::Map<_, _>>>()?;
        ensure!(
            Value::Object(params) == row["params"],
            "read {name}: source/catalog params mismatch"
        );
    }
    Ok(())
}

pub(super) fn document(root: &Path, pins: &DocumentMut, _tables: &Value) -> Result<Value> {
    let mut resolver = Resolver::new(root)?;
    let (catalog, catalog_provenance) = catalog(root, pins)?;
    let (rules, own_key) = requiredness(&mut resolver)?;
    let base = Source::load(&resolver, BASE)?;
    let fetch = scalar_factory(&base)?;
    let registration = registration_rule(&base)?;
    let (paths, module_provenance) = module_paths(&resolver)?;
    let mut reads = BTreeMap::new();
    for path in paths {
        let source = Source::load(&resolver, &path)?;
        extract_module(&mut resolver, &source, &base, fetch, &own_key, &mut reads)?;
    }
    compare_catalog(&reads, &catalog)?;
    Ok(
        json!({"format":1,"parity":{"tag":pins["parity"]["tag"].as_str(),"commit":pins["parity"]["commit"].as_str()},"catalog":catalog_provenance,"requiredness":rules,"registration":registration,"modules":module_provenance,"reads":reads.into_values().collect::<Vec<_>>()}),
    )
}

pub(super) fn outputs(
    root: &Path,
    pins: &DocumentMut,
    tables: &Value,
) -> Result<Vec<(&'static str, Vec<u8>)>> {
    Ok(vec![(OUTPUT, pretty(&document(root, pins, tables)?)?)])
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
    fn pinned_read_params_preserve_defaults_requiredness_and_spans() -> Result<()> {
        let root = root()?;
        let pins = fs::read_to_string(root.join("upstream.toml"))?.parse::<DocumentMut>()?;
        let doc = document(&root, &pins, &Value::Null)?;
        let reads = doc["reads"].as_array().context("reads missing")?;
        assert_eq!(reads.len(), READ_COUNT);
        let param = |read: &str, name: &str| -> Result<&Value> {
            reads
                .iter()
                .find(|row| row["name"] == read)
                .and_then(|row| row["params"].as_array())
                .and_then(|params| params.iter().find(|param| param["name"] == name))
                .context("read param missing")
        };
        assert_eq!(
            param("weights", "mechid")?["default"],
            json!({"present":true,"value":0})
        );
        assert_eq!(param("weights", "mechid")?["required"], false);
        assert_eq!(
            param("neurons", "lite")?["default"],
            json!({"present":true,"value":true})
        );
        assert_eq!(
            param("neurons", "netuid")?["default"],
            json!({"present":false})
        );
        assert_eq!(param("neurons", "netuid")?["required"], true);
        assert_eq!(
            param("difficulty", "netuid")?["default"],
            json!({"present":true,"value":null})
        );
        assert_eq!(param("difficulty", "netuid")?["required"], true);
        assert_eq!(
            param("miner_collateral", "coldkey_ss58")?["default"],
            json!({"present":true,"value":null})
        );
        assert_eq!(
            param("balance", "coldkey_ss58")?["default"],
            json!({"present":false})
        );
        assert_eq!(param("balance", "coldkey_ss58")?["own_key"], true);
        assert_eq!(param("balance", "coldkey_ss58")?["required"], false);
        assert_eq!(param("balances", "coldkey_ss58s")?["required"], true);
        assert!(
            reads
                .iter()
                .find(|read| read["name"] == "tx_rate_limit")
                .context("scalar read missing")?["params"]
                .as_array()
                .context("params missing")?
                .is_empty()
        );
        for read in reads {
            for key in ["declaration", "fetch"] {
                let span = &read["provenance"][key];
                assert_eq!(span["blob"].as_str().context("blob missing")?.len(), 40);
                assert!(span["start_byte"].as_u64() < span["end_byte"].as_u64());
            }
        }
        Ok(())
    }

    #[test]
    fn requiredness_rejects_semantic_changes_and_rebindings() -> Result<()> {
        let root = root()?;
        let mut resolver = Resolver::new(&root)?;
        let source = resolver.source(QUERY)?;
        for changed in [
            source.replace(
                "return default is inspect.Parameter.empty or default is None",
                "return default is inspect.Parameter.empty",
            ),
            source.replace(
                "return False\n    default = _fetch_default",
                "return True\n    default = _fetch_default",
            ),
            source.replace("return fetch_params[pname].default", "return None"),
            format!("{source}\nif enabled:\n    _OWN_KEY = ()\n"),
            format!("{source}\n_is_required = lambda *args: False\n"),
        ] {
            resolver.sources.insert(QUERY.into(), changed);
            assert!(requiredness(&mut resolver).is_err());
        }
        Ok(())
    }

    #[test]
    fn scalar_and_decorator_rules_reject_changed_registration_semantics() -> Result<()> {
        let root = root()?;
        let mut resolver = Resolver::new(&root)?;
        let source = resolver.source(BASE)?;
        for changed in [
            source.replace("netuid: Optional[int] = None", "netuid: Optional[int] = 0"),
            source.replace(
                "params = {\"netuid\": \"integer\"} if per_netuid else {}",
                "params = {\"netuid\": \"string\"} if per_netuid else {}",
            ),
            source.replace(
                "read(name, params, category=category, param_docs=param_docs)(fetch)",
                "read(name, {}, category=category, param_docs=param_docs)(fetch)",
            ),
        ] {
            resolver.sources.insert(BASE.into(), changed);
            assert!(scalar_factory(&Source::load(&resolver, BASE)?).is_err());
        }
        for changed in [
            source.replace(
                "registered = _coercing(fn, params or {})",
                "registered = fn",
            ),
            source.replace("@functools.wraps(fn)", "@functools.wraps(other)"),
            source.replace(
                "name, doc, params or {}, registered, category",
                "name, doc, {}, registered, category",
            ),
        ] {
            resolver.sources.insert(BASE.into(), changed);
            assert!(registration_rule(&Source::load(&resolver, BASE)?).is_err());
        }
        Ok(())
    }

    #[test]
    fn read_catalog_and_module_discovery_reject_drift_and_duplicates() -> Result<()> {
        let root = root()?;
        let pins = fs::read_to_string(root.join("upstream.toml"))?.parse::<DocumentMut>()?;
        let doc = document(&root, &pins, &Value::Null)?;
        let reads = doc["reads"]
            .as_array()
            .context("reads missing")?
            .iter()
            .map(|read| {
                Ok((
                    read["name"].as_str().context("name missing")?.to_owned(),
                    read.clone(),
                ))
            })
            .collect::<Result<BTreeMap<_, _>>>()?;
        let (catalog, _) = catalog(&root, &pins)?;
        let mut changed = catalog.clone();
        changed[0]["params"]["coldkey_ss58"] = json!("integer");
        assert!(compare_catalog(&reads, &changed).is_err());
        let mut changed = catalog;
        changed[1] = changed[0].clone();
        assert!(compare_catalog(&reads, &changed).is_err());
        let mut changed = reads;
        let duplicate = changed.values().next().context("read missing")?.clone();
        assert!(insert_read(&mut changed, duplicate).is_err());

        let mut resolver = Resolver::new(&root)?;
        let source = resolver.source(INIT)?;
        resolver
            .sources
            .insert(INIT.into(), source.replace("    hyperparameters,", ""));
        assert!(module_paths(&resolver).is_err());
        let path = "sdk/python/bittensor/reads/accounts.py";
        let source = resolver.source(path)?;
        resolver.sources.insert(
            path.into(),
            format!("{source}\nif enabled:\n    read = other\n"),
        );
        assert!(module_imports(&Source::load(&resolver, path)?).is_err());
        Ok(())
    }

    #[test]
    fn read_modules_reject_hidden_registrations_and_registry_mutation() -> Result<()> {
        let root = root()?;
        let mut resolver = Resolver::new(&root)?;
        let base = Source::load(&resolver, BASE)?;
        let fetch = scalar_factory(&base)?;
        let own_key = ["hotkey_ss58".to_owned(), "coldkey_ss58".to_owned()]
            .into_iter()
            .collect();
        let path = "sdk/python/bittensor/reads/accounts.py";
        let original = resolver.source(path)?;
        for suffix in [
            "\nalias = read\n",
            "\nalias = lambda: read('hidden', {}, category='Accounts & keys')(balances)\n",
            "\nhidden = read('hidden', {}, category='Accounts & keys')(balances)\n",
            "\nclass Hidden:\n    @read('hidden', {}, category='Accounts & keys')\n    async def fetch(view):\n        return None\n",
            "\nremoved = REGISTRY.pop('balance')\n",
        ] {
            resolver
                .sources
                .insert(path.into(), format!("{original}{suffix}"));
            let source = Source::load(&resolver, path)?;
            let mut reads = BTreeMap::new();
            assert!(
                extract_module(&mut resolver, &source, &base, fetch, &own_key, &mut reads,)
                    .is_err()
            );
        }
        let original = resolver.source(INIT)?;
        resolver
            .sources
            .insert(INIT.into(), format!("{original}\nREGISTRY.clear()\n"));
        assert!(module_paths(&resolver).is_err());
        Ok(())
    }
}
