//! Authenticated Python environment reads and Typer environment bindings.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use anyhow::{Context, Result, bail, ensure};
use rustpython_ast::Visitor;
use rustpython_parser::{
    Parse,
    ast::{self, Ranged},
};
use serde_json::{Value, json};
use toml_edit::DocumentMut;

use super::{Binding, Bindings, Literal, Resolver, call_path, line, pretty};

const PY: &str = "sdk/python/bittensor/";
const KEYFILES: &str = "sdk/python/bittensor/keyfiles.py";
const CONFIG: &str = "sdk/python/bittensor/config.py";

#[derive(Clone)]
struct Scope {
    aliases: BTreeMap<String, String>,
    names: BTreeSet<String>,
    bindings: Bindings,
    imports: BTreeMap<String, (ast::StmtImportFrom, String)>,
    uncertain_aliases: BTreeSet<String>,
}

fn source_span(
    source: &str,
    path: &str,
    blob: Option<&String>,
    range: ast::text_size::TextRange,
) -> Value {
    let start = range.start().to_usize();
    let end = range.end().to_usize();
    json!({"path":path,"blob":blob,"line":line(source,start),"end_line":line(source,end),"start_byte":start,"end_byte":end})
}

fn import_name(path: &str, import: &ast::StmtImportFrom, name: &str) -> Result<String> {
    let module = import.module.as_ref().map_or("", |name| name.as_str());
    let level = import.level.map_or(0, |level| level.to_usize());
    if level == 0 {
        if module.starts_with("bittensor.") || module == "bittensor" {
            return Ok(format!("sdk/python/{}.py.{name}", module.replace('.', "/")));
        }
        return Ok(format!("{module}.{name}"));
    }
    let mut base = Path::new(path)
        .parent()
        .context("Python import has no parent")?
        .to_path_buf();
    for _ in 1..level {
        ensure!(base.pop(), "relative Python import escapes its root");
    }
    if module.is_empty() {
        return Ok(base.join(name).with_extension("py").display().to_string());
    }
    let module = base.join(module.replace('.', "/"));
    Ok(format!("{}.py.{name}", module.display()))
}

struct Scanner<'a, 'root> {
    resolver: &'a mut Resolver<'root>,
    path: &'a str,
    source: &'a str,
    bindings: Bindings,
    imports: BTreeMap<String, (ast::StmtImportFrom, String)>,
    aliases: BTreeMap<String, String>,
    export_aliases: BTreeMap<String, BTreeMap<String, String>>,
    local_names: BTreeSet<String>,
    class_outer: Vec<Scope>,
    uncertain_aliases: BTreeSet<String>,
    occurrences: Vec<Value>,
    fallbacks: BTreeMap<usize, Value>,
    error: Option<anyhow::Error>,
    depth: usize,
    nodes: usize,
}

impl Scanner<'_, '_> {
    fn canonical(&self, expression: &ast::Expr) -> Option<String> {
        canonical(expression, &self.aliases)
    }

    fn api(&mut self, expression: &ast::Expr) -> Result<Option<String>> {
        if let Some(path) = call_path(expression) {
            let name = path.split('.').next().unwrap_or("");
            ensure!(
                !self.uncertain_aliases.contains(name),
                "conditional rebinding prevents environment namespace resolution for {name}"
            );
        }
        let Some(mut symbol) = self.canonical(expression) else {
            return Ok(None);
        };
        let mut seen = BTreeSet::new();
        for _ in 0..super::super::MAX_DEPTH {
            if !symbol.starts_with(PY) {
                return Ok(Some(symbol));
            }
            let Some((module, attributes)) = symbol.split_once(".py.") else {
                return Ok(Some(symbol));
            };
            let module = format!("{module}.py");
            let package = Path::new(&module)
                .with_extension("")
                .join("__init__.py")
                .display()
                .to_string();
            let module = if !self.resolver.tree.contains_key(&module)
                && self.resolver.tree.contains_key(&package)
            {
                package
            } else {
                module
            };
            let exists = self.resolver.tree.contains_key(&module);
            #[cfg(test)]
            let exists = exists || self.resolver.sources.contains_key(&module);
            if !exists {
                return Ok(Some(symbol));
            }
            ensure!(
                seen.insert(symbol.clone()),
                "cyclic environment namespace alias"
            );
            if !self.export_aliases.contains_key(&module) {
                let source = self.resolver.source(&module)?;
                let aliases = export_aliases(&source, &module)?;
                self.export_aliases.insert(module.clone(), aliases);
            }
            let (name, suffix) = attributes
                .split_once('.')
                .map_or((attributes, None), |(name, suffix)| (name, Some(suffix)));
            let Some(resolved) = self.export_aliases[&module].get(name) else {
                return Ok(Some(symbol));
            };
            symbol =
                suffix.map_or_else(|| resolved.clone(), |suffix| format!("{resolved}.{suffix}"));
        }
        bail!("environment namespace resolution exceeds depth bound")
    }

    fn span(&self, range: ast::text_size::TextRange) -> Value {
        source_span(
            self.source,
            self.path,
            self.resolver.tree.get(self.path),
            range,
        )
    }
}

fn canonical(expression: &ast::Expr, aliases: &BTreeMap<String, String>) -> Option<String> {
    match expression {
        ast::Expr::Name(name) => aliases.get(name.id.as_str()).cloned(),
        ast::Expr::Attribute(attr) => {
            canonical(&attr.value, aliases).map(|base| format!("{base}.{}", attr.attr))
        }
        _ => None,
    }
}

fn export_aliases(source: &str, path: &str) -> Result<BTreeMap<String, String>> {
    let suite =
        ast::Suite::parse(source, path).map_err(|error| anyhow::anyhow!("{path}: {error}"))?;
    let mut aliases = BTreeMap::new();
    for statement in suite {
        match statement {
            ast::Stmt::Import(import) => {
                for alias in import.names {
                    let local = alias.asname.as_ref().map_or_else(
                        || alias.name.split('.').next().unwrap_or(""),
                        |name| name.as_str(),
                    );
                    let target = if alias.asname.is_some() {
                        alias.name.as_str()
                    } else {
                        local
                    };
                    aliases.insert(local.to_owned(), target.to_owned());
                }
            }
            ast::Stmt::ImportFrom(import) => {
                for alias in &import.names {
                    ensure!(
                        alias.name.as_str() != "*",
                        "wildcard import prevents environment export resolution"
                    );
                    aliases.insert(
                        alias.asname.as_ref().unwrap_or(&alias.name).to_string(),
                        import_name(path, &import, alias.name.as_str())?,
                    );
                }
            }
            ast::Stmt::Assign(assign) => {
                let alias = canonical(&assign.value, &aliases);
                for target in assign.targets {
                    let mut stores = Stores::default();
                    stores.visit_expr(target.clone());
                    for name in stores.0 {
                        aliases.remove(&name);
                    }
                    if let ast::Expr::Name(name) = target
                        && let Some(alias) = &alias
                    {
                        aliases.insert(name.id.to_string(), alias.clone());
                    }
                }
            }
            ast::Stmt::AnnAssign(assign) => {
                let alias = assign
                    .value
                    .as_deref()
                    .and_then(|value| canonical(value, &aliases));
                let mut stores = Stores::default();
                stores.visit_expr(*assign.target.clone());
                for name in stores.0 {
                    aliases.remove(&name);
                }
                if let ast::Expr::Name(name) = *assign.target
                    && let Some(alias) = alias
                {
                    aliases.insert(name.id.to_string(), alias);
                }
            }
            ast::Stmt::FunctionDef(def) => {
                aliases.remove(def.name.as_str());
            }
            ast::Stmt::AsyncFunctionDef(def) => {
                aliases.remove(def.name.as_str());
            }
            ast::Stmt::ClassDef(def) => {
                aliases.remove(def.name.as_str());
            }
            statement => {
                for name in super::super::effects::conditional_names(&[statement]) {
                    aliases.remove(&name);
                }
            }
        }
    }
    Ok(aliases)
}

impl Scanner<'_, '_> {
    fn snapshot(&self) -> Scope {
        Scope {
            aliases: self.aliases.clone(),
            names: self.local_names.clone(),
            bindings: self.bindings.clone(),
            imports: self.imports.clone(),
            uncertain_aliases: self.uncertain_aliases.clone(),
        }
    }

    fn restore(&mut self, scope: Scope) {
        self.aliases = scope.aliases;
        self.local_names = scope.names;
        self.bindings = scope.bindings;
        self.imports = scope.imports;
        self.uncertain_aliases = scope.uncertain_aliases;
    }
    fn expression(&self, expression: &ast::Expr) -> Value {
        json!({"expression": &self.source[expression.range().start().to_usize()..expression.range().end().to_usize()],"source":self.span(expression.range())})
    }

    fn bounded_assignment(&self, expression: &ast::Expr) -> Result<bool> {
        let mut bounded = true;
        super::super::effects::walk(expression, 0, &mut |expression| match expression {
            ast::Expr::Call(call) => {
                let symbol = self.canonical(&call.func).or_else(|| call_path(&call.func));
                bounded &= symbol.is_some_and(|name| {
                    matches!(
                        name.as_str(),
                        "pathlib.Path.home"
                            | "str"
                            | "int"
                            | "float"
                            | "bool"
                            | "bytes"
                            | "list"
                            | "tuple"
                            | "dict"
                            | "set"
                            | "frozenset"
                    )
                });
            }
            ast::Expr::Lambda(_)
            | ast::Expr::ListComp(_)
            | ast::Expr::SetComp(_)
            | ast::Expr::DictComp(_)
            | ast::Expr::GeneratorExp(_)
            | ast::Expr::NamedExpr(_)
            | ast::Expr::Await(_)
            | ast::Expr::Yield(_)
            | ast::Expr::YieldFrom(_) => bounded = false,
            _ => {}
        })?;
        Ok(bounded)
    }

    fn literal(&mut self, expression: &ast::Expr) -> Result<Literal> {
        let names = super::super::effects::names(expression)?;
        ensure!(
            !names
                .iter()
                .any(|name| self.local_names.contains(name) && !self.imports.contains_key(name)),
            "environment default/name depends on a local binding"
        );
        for name in names {
            if !self.bindings.contains_key(&name)
                && let Some((import, original)) = self.imports.get(&name).cloned()
            {
                let binding = self
                    .resolver
                    .imported_binding(self.path, &import, &original)?;
                self.bindings.insert(name, binding);
            }
        }
        self.resolver.eval(expression, &self.bindings, self.path, 0)
    }

    fn default(&mut self, expression: Option<&ast::Expr>) -> Result<Value> {
        let Some(expression) = expression else {
            return Ok(json!({"kind":"missing"}));
        };
        if let ast::Expr::Call(call) = expression
            && self.canonical(&call.func).as_deref() == Some("sdk/python/bittensor/config.py.get")
        {
            ensure!(
                (1..=2).contains(&call.args.len()) && call.keywords.is_empty(),
                "config default requires a key and optional bounded fallback"
            );
            let Literal::Str(key) = self.literal(&call.args[0])? else {
                bail!("config key must resolve to a string");
            };
            let fallback = if let Some(value) = call.args.get(1) {
                self.default(Some(value))?
            } else {
                json!({"kind":"literal","value":null})
            };
            return Ok(
                json!({"kind":"config_lookup","key":key,"fallback":fallback,"source":self.span(expression.range()),"resolver":config_lookup(self.resolver)?}),
            );
        }
        let literal = self.literal(expression).with_context(|| {
            format!(
                "{}:{}: environment default is outside bounded resolution",
                self.path,
                line(self.source, expression.range().start().to_usize())
            )
        })?;
        Ok(
            json!({"kind":if matches!(literal,Literal::Resolved(_)) {"resolved"} else {"literal"},"value":literal.json()?,"source":self.span(expression.range())}),
        )
    }

    fn names(&mut self, expression: &ast::Expr, multiple: bool) -> Result<Vec<String>> {
        let literal = self.literal(expression).with_context(|| {
            format!(
                "{}:{}: environment name must be a bounded string",
                self.path,
                line(self.source, expression.range().start().to_usize())
            )
        })?;
        let values = match literal {
            Literal::Str(value) => vec![value],
            Literal::List(values) | Literal::Tuple(values) if multiple => values
                .into_iter()
                .map(|value| {
                    if let Literal::Str(value) = value {
                        Ok(value)
                    } else {
                        bail!("Typer envvar list contains a non-string")
                    }
                })
                .collect::<Result<Vec<_>>>()?,
            _ => bail!("environment name is not a string"),
        };
        ensure!(
            !values.is_empty()
                && values
                    .iter()
                    .all(|value| !value.is_empty() && !value.contains(['=', '\0'])),
            "invalid environment name"
        );
        Ok(values)
    }

    fn record(
        &mut self,
        name: String,
        api: &str,
        default: Value,
        range: ast::text_size::TextRange,
    ) -> Result<()> {
        ensure!(
            self.occurrences.len() < super::super::MAX_CONTAINER_ITEMS,
            "environment occurrences exceed inventory bound"
        );
        let fallback = self.fallbacks.get(&range.start().to_usize());
        let row = json!({"name":name,"api":api,"default":default,"missing_value":if api=="os.environ.__getitem__" {"raises_key_error"} else if api.starts_with("typer.") {"uses_parameter_default"} else {"returns_default_or_none"},"empty_value":if api.starts_with("typer.") {"treated_as_unset_by_click"} else if api=="bittensor_core.get_password_from_environment" {"native_password_decode"} else {"returned_unchanged"},"truthy_fallback":fallback,"source":self.span(range)});
        super::super::spend(&self.resolver.budget, row.to_string().len())?;
        self.occurrences.push(row);
        Ok(())
    }

    fn inspect_call(&mut self, call: &ast::ExprCall) -> Result<()> {
        let Some(api) = self.api(&call.func)? else {
            return Ok(());
        };
        if matches!(api.as_str(), "os.getenv" | "os.environ.get") {
            ensure!(
                call.args.len() <= 2
                    && call.keywords.iter().all(|keyword| keyword
                        .arg
                        .as_ref()
                        .is_some_and(|name| matches!(name.as_str(), "key" | "default"))),
                "unsupported environment read signature"
            );
            let key_kw = call
                .keywords
                .iter()
                .find(|keyword| keyword.arg.as_ref().is_some_and(|name| name == "key"));
            let default_kw = call
                .keywords
                .iter()
                .find(|keyword| keyword.arg.as_ref().is_some_and(|name| name == "default"));
            ensure!(
                (key_kw.is_none() || call.args.is_empty())
                    && (default_kw.is_none() || call.args.len() != 2),
                "duplicated environment argument"
            );
            let key = call
                .args
                .first()
                .or_else(|| key_kw.map(|keyword| &keyword.value))
                .context("environment read has no key")?;
            let names = self.names(key, false)?;
            let default = self.default(
                call.args
                    .get(1)
                    .or_else(|| default_kw.map(|keyword| &keyword.value)),
            )?;
            for name in names {
                self.record(name, &api, default.clone(), call.range)?;
            }
        } else if api == "bittensor_core.get_password_from_environment" {
            ensure!(
                call.args.len() == 1 && call.keywords.is_empty(),
                "native environment-password read signature changed"
            );
            if self.path == KEYFILES && call_path(&call.args[0]).as_deref() == Some("env_var_name")
            {
                let family = password_family(self.resolver)?;
                ensure!(
                    family["source"]["start_byte"].as_u64()
                        == Some(call.range.start().to_usize() as u64),
                    "unexpected dynamic native environment-password read"
                );
            } else {
                for name in self.names(&call.args[0], false)? {
                    self.record(name, &api, json!({"kind":"missing"}), call.range)?;
                }
            }
        } else if matches!(api.as_str(), "typer.Option" | "typer.Argument") {
            for keyword in &call.keywords {
                if keyword.arg.as_ref().is_some_and(|name| name == "envvar") {
                    let names = self.names(&keyword.value, true)?;
                    let default_kw = call
                        .keywords
                        .iter()
                        .find(|keyword| keyword.arg.as_ref().is_some_and(|name| name == "default"));
                    ensure!(
                        default_kw.is_none() || call.args.is_empty(),
                        "Typer parameter duplicates default"
                    );
                    let default = self.default(
                        call.args
                            .first()
                            .or_else(|| default_kw.map(|keyword| &keyword.value)),
                    )?;
                    for name in names {
                        self.record(name, &api, default.clone(), call.range)?;
                    }
                }
            }
        }
        Ok(())
    }

    fn inspect_subscript(&mut self, subscript: &ast::ExprSubscript) -> Result<()> {
        if subscript.ctx == ast::ExprContext::Load
            && self.api(&subscript.value)?.as_deref() == Some("os.environ")
        {
            for name in self.names(&subscript.slice, false)? {
                self.record(
                    name,
                    "os.environ.__getitem__",
                    json!({"kind":"missing"}),
                    subscript.range,
                )?;
            }
        }
        Ok(())
    }

    fn scope(&mut self, args: ast::Arguments, body: Vec<ast::Stmt>) {
        // Python evaluates defaults and annotations in the enclosing scope.
        self.visit_arguments(args.clone());
        let old_aliases = self.aliases.clone();
        let old_names = self.local_names.clone();
        let old_bindings = self.bindings.clone();
        let old_imports = self.imports.clone();
        let old_uncertain = self.uncertain_aliases.clone();
        let classes = std::mem::take(&mut self.class_outer);
        if let Some(outer) = classes.last() {
            self.restore(outer.clone());
        }
        let mut names = super::super::effects::conditional_names(&body);
        for arg in args
            .posonlyargs
            .iter()
            .chain(&args.args)
            .chain(&args.kwonlyargs)
        {
            names.insert(arg.def.arg.to_string());
        }
        for arg in [args.vararg, args.kwarg].into_iter().flatten() {
            names.insert(arg.arg.to_string());
        }
        for name in &names {
            self.aliases.remove(name);
            self.imports.remove(name);
            self.bindings.remove(name);
            self.uncertain_aliases.remove(name);
        }
        self.local_names.extend(names);
        for statement in body {
            self.visit_stmt(statement);
        }
        self.aliases = old_aliases;
        self.local_names = old_names;
        self.class_outer = classes;
        self.bindings = old_bindings;
        self.imports = old_imports;
        self.uncertain_aliases = old_uncertain;
    }

    fn comprehension(&mut self, generators: Vec<ast::Comprehension>, expressions: Vec<ast::Expr>) {
        let old = self.snapshot();
        for generator in generators {
            self.visit_expr(generator.iter);
            let mut stores = Stores::default();
            stores.visit_expr(generator.target.clone());
            for name in stores.0 {
                self.aliases.remove(&name);
                self.local_names.insert(name);
            }
            self.visit_expr(generator.target);
            for condition in generator.ifs {
                self.visit_expr(condition);
            }
        }
        for expression in expressions {
            self.visit_expr(expression);
        }
        self.restore(old);
    }

    fn invalidate_tables(&mut self) {
        fn mutable(value: &Literal) -> bool {
            match value {
                Literal::List(_) | Literal::Dict(_) | Literal::Set(_) => true,
                Literal::Tuple(values) | Literal::FrozenSet(values) => values.iter().any(mutable),
                _ => false,
            }
        }
        self.bindings
            .retain(|_, binding| !matches!(binding,Binding::Value(value) if mutable(value)));
    }

    fn save_error(&mut self, result: Result<()>) {
        if self.error.is_none() {
            self.error = result.err();
        }
    }
}

#[derive(Default)]
struct Stores(BTreeSet<String>);

impl Visitor for Stores {
    fn visit_expr_name(&mut self, node: ast::ExprName) {
        if node.ctx != ast::ExprContext::Load {
            self.0.insert(node.id.to_string());
        }
    }
}

impl Visitor for Scanner<'_, '_> {
    fn visit_arguments(&mut self, node: ast::Arguments) {
        for argument in node
            .posonlyargs
            .into_iter()
            .chain(node.args)
            .chain(node.kwonlyargs)
        {
            self.visit_arg(argument.def);
            if let Some(default) = argument.default {
                self.visit_expr(*default);
            }
        }
        for argument in [node.vararg, node.kwarg].into_iter().flatten() {
            self.visit_arg(*argument);
        }
    }

    fn visit_arg(&mut self, node: ast::Arg) {
        if let Some(annotation) = node.annotation {
            self.visit_expr(*annotation);
        }
    }

    fn visit_keyword(&mut self, node: ast::Keyword) {
        self.visit_expr(node.value);
    }

    fn visit_comprehension(&mut self, node: ast::Comprehension) {
        self.visit_expr(node.iter);
        self.visit_expr(node.target);
        for condition in node.ifs {
            self.visit_expr(condition);
        }
    }

    fn visit_expr_list_comp(&mut self, node: ast::ExprListComp) {
        self.comprehension(node.generators, vec![*node.elt]);
    }

    fn visit_expr_set_comp(&mut self, node: ast::ExprSetComp) {
        self.comprehension(node.generators, vec![*node.elt]);
    }

    fn visit_expr_dict_comp(&mut self, node: ast::ExprDictComp) {
        self.comprehension(node.generators, vec![*node.key, *node.value]);
    }

    fn visit_expr_generator_exp(&mut self, node: ast::ExprGeneratorExp) {
        self.comprehension(node.generators, vec![*node.elt]);
    }

    fn visit_expr_name(&mut self, node: ast::ExprName) {
        if node.ctx != ast::ExprContext::Load {
            self.aliases.remove(node.id.as_str());
            self.bindings.remove(node.id.as_str());
            self.imports.remove(node.id.as_str());
            self.uncertain_aliases.remove(node.id.as_str());
        }
    }

    fn visit_withitem(&mut self, node: ast::WithItem) {
        self.visit_expr(node.context_expr);
        if let Some(target) = node.optional_vars {
            self.visit_expr(*target);
        }
    }

    fn visit_match_case(&mut self, node: ast::MatchCase) {
        self.visit_pattern(node.pattern);
        if let Some(guard) = node.guard {
            self.visit_expr(*guard);
        }
        for statement in node.body {
            self.visit_stmt(statement);
        }
    }

    fn visit_stmt(&mut self, node: ast::Stmt) {
        if self.error.is_some() {
            return;
        }
        self.nodes += 1;
        self.depth += 1;
        let conditional = matches!(
            node,
            ast::Stmt::If(_)
                | ast::Stmt::For(_)
                | ast::Stmt::AsyncFor(_)
                | ast::Stmt::While(_)
                | ast::Stmt::Try(_)
                | ast::Stmt::TryStar(_)
                | ast::Stmt::With(_)
                | ast::Stmt::AsyncWith(_)
                | ast::Stmt::Match(_)
        );
        let changes = if conditional {
            super::super::effects::conditional_names(std::slice::from_ref(&node))
        } else {
            BTreeSet::new()
        };
        let old_aliases = self.aliases.clone();
        if self.depth >= super::super::MAX_DEPTH || self.nodes > 1_000_000 {
            self.error = Some(anyhow::anyhow!(
                "environment inventory traversal exceeds bounds"
            ));
        } else {
            self.generic_visit_stmt(node);
        }
        for name in changes {
            let environmental = old_aliases
                .get(&name)
                .into_iter()
                .chain(self.aliases.get(&name))
                .any(|alias| {
                    alias == "os"
                        || alias.starts_with("os.")
                        || alias == "typer"
                        || alias.starts_with("typer.")
                });
            if environmental {
                self.uncertain_aliases.insert(name.clone());
            }
            self.aliases.remove(&name);
            self.bindings.remove(&name);
            self.imports.remove(&name);
        }
        self.depth -= 1;
    }

    fn visit_stmt_if(&mut self, node: ast::StmtIf) {
        self.visit_expr(*node.test);
        let scope = self.snapshot();
        for statement in node.body {
            self.visit_stmt(statement);
        }
        self.restore(scope.clone());
        for statement in node.orelse {
            self.visit_stmt(statement);
        }
        self.restore(scope);
    }

    fn visit_excepthandler_except_handler(&mut self, node: ast::ExceptHandlerExceptHandler) {
        if let Some(expression) = node.type_ {
            self.visit_expr(*expression);
        }
        if let Some(name) = node.name {
            self.aliases.remove(name.as_str());
            self.bindings.remove(name.as_str());
            self.imports.remove(name.as_str());
            self.uncertain_aliases.remove(name.as_str());
        }
        for statement in node.body {
            self.visit_stmt(statement);
        }
    }

    fn visit_pattern_match_as(&mut self, node: ast::PatternMatchAs) {
        if let Some(name) = node.name {
            self.aliases.remove(name.as_str());
            self.bindings.remove(name.as_str());
            self.imports.remove(name.as_str());
            self.uncertain_aliases.remove(name.as_str());
        }
        if let Some(pattern) = node.pattern {
            self.visit_pattern(*pattern);
        }
    }

    fn visit_pattern_match_star(&mut self, node: ast::PatternMatchStar) {
        if let Some(name) = node.name {
            self.aliases.remove(name.as_str());
            self.bindings.remove(name.as_str());
            self.imports.remove(name.as_str());
            self.uncertain_aliases.remove(name.as_str());
        }
    }

    fn visit_stmt_match(&mut self, node: ast::StmtMatch) {
        self.visit_expr(*node.subject);
        let old = self.snapshot();
        for case in node.cases {
            self.restore(old.clone());
            self.visit_match_case(case);
        }
        self.restore(old);
    }

    fn visit_expr(&mut self, node: ast::Expr) {
        if self.error.is_some() {
            return;
        }
        self.nodes += 1;
        self.depth += 1;
        if self.depth >= super::super::MAX_DEPTH || self.nodes > 1_000_000 {
            self.error = Some(anyhow::anyhow!(
                "environment inventory traversal exceeds bounds"
            ));
        } else {
            self.generic_visit_expr(node);
        }
        self.depth -= 1;
    }

    fn visit_stmt_import(&mut self, node: ast::StmtImport) {
        for alias in node.names {
            let local = alias.asname.as_ref().map_or_else(
                || alias.name.split('.').next().unwrap_or(""),
                |name| name.as_str(),
            );
            let target = if alias.asname.is_some() {
                alias.name.as_str()
            } else {
                local
            };
            self.aliases.insert(local.to_owned(), target.to_owned());
            self.bindings
                .insert(local.to_owned(), Binding::Module(target.to_owned()));
            self.imports.remove(local);
        }
    }

    fn visit_stmt_import_from(&mut self, node: ast::StmtImportFrom) {
        for alias in &node.names {
            if alias.name.as_str() == "*" {
                self.error = Some(anyhow::anyhow!(
                    "wildcard import prevents environment alias resolution"
                ));
                return;
            }
            match import_name(self.path, &node, alias.name.as_str()) {
                Ok(target) => {
                    let local = alias.asname.as_ref().unwrap_or(&alias.name).to_string();
                    self.aliases.insert(local.clone(), target.clone());
                    self.bindings.remove(&local);
                    if node.level.map_or(0, |level| level.to_usize()) == 0
                        && node.module.as_ref().is_some_and(|module| {
                            matches!(module.as_str(), "os" | "typer" | "pathlib")
                        })
                    {
                        self.bindings.insert(local.clone(), Binding::Symbol(target));
                    }
                    let mut single = node.clone();
                    single.names = vec![alias.clone()];
                    self.imports.insert(local, (single, alias.name.to_string()));
                }
                Err(error) => {
                    self.error = Some(error);
                    return;
                }
            }
        }
    }

    fn visit_stmt_assign(&mut self, node: ast::StmtAssign) {
        let alias = self.canonical(&node.value);
        let module_value=node.targets.iter().all(|target|matches!(target,ast::Expr::Name(name) if !self.local_names.contains(name.id.as_str())));
        let value = if module_value && self.bounded_assignment(&node.value).unwrap_or(false) {
            self.literal(&node.value)
                .map(Binding::Value)
                .or_else(|_| {
                    self.resolver
                        .eval_binding(&node.value, &self.bindings, self.path, 0)
                })
                .unwrap_or_else(|error| Binding::Refused(format!("{error:#}")))
        } else {
            Binding::Refused("not a bounded module-level environment constant".into())
        };
        self.visit_expr(*node.value);
        for target in node.targets {
            if !matches!(target, ast::Expr::Name(_))
                && let Some(path) = match &target {
                    ast::Expr::Subscript(subscript) => call_path(&subscript.value),
                    ast::Expr::Attribute(attr) => call_path(&attr.value),
                    _ => None,
                }
            {
                let name = path.split('.').next().unwrap_or("");
                self.bindings.remove(name);
                self.imports.remove(name);
                self.invalidate_tables();
                if matches!(target, ast::Expr::Attribute(_))
                    && self.aliases.get(name).is_some_and(|alias| {
                        alias == "os"
                            || alias.starts_with("os.")
                            || alias == "typer"
                            || alias.starts_with("typer.")
                    })
                {
                    self.uncertain_aliases.insert(name.to_owned());
                    self.aliases.remove(name);
                }
            }
            let name = if let ast::Expr::Name(name) = &target {
                Some(name.id.to_string())
            } else {
                None
            };
            self.visit_expr(target);
            if let Some(name) = name {
                self.aliases.remove(&name);
                self.imports.remove(&name);
                self.bindings.insert(name.clone(), value.clone());
                if let Some(alias) = &alias {
                    self.aliases.insert(name, alias.clone());
                }
            }
        }
    }

    fn visit_stmt_ann_assign(&mut self, node: ast::StmtAnnAssign) {
        self.visit_expr(*node.annotation);
        if let Some(value) = node.value {
            self.visit_stmt_assign(ast::StmtAssign {
                range: node.range,
                targets: vec![*node.target],
                value,
                type_comment: None,
            });
        } else {
            self.visit_expr(*node.target);
        }
    }

    fn visit_stmt_function_def(&mut self, node: ast::StmtFunctionDef) {
        for expression in node.decorator_list {
            self.visit_expr(expression);
        }
        if let Some(returns) = node.returns {
            self.visit_expr(*returns);
        }
        self.scope(*node.args, node.body);
        self.aliases.remove(node.name.as_str());
        self.bindings.remove(node.name.as_str());
        self.imports.remove(node.name.as_str());
    }

    fn visit_stmt_async_function_def(&mut self, node: ast::StmtAsyncFunctionDef) {
        for expression in node.decorator_list {
            self.visit_expr(expression);
        }
        if let Some(returns) = node.returns {
            self.visit_expr(*returns);
        }
        self.scope(*node.args, node.body);
        self.aliases.remove(node.name.as_str());
        self.bindings.remove(node.name.as_str());
        self.imports.remove(node.name.as_str());
    }

    fn visit_expr_lambda(&mut self, node: ast::ExprLambda) {
        self.scope(
            *node.args,
            vec![ast::Stmt::Expr(ast::StmtExpr {
                range: node.range,
                value: node.body,
            })],
        );
    }

    fn visit_stmt_class_def(&mut self, node: ast::StmtClassDef) {
        for expression in node
            .decorator_list
            .into_iter()
            .chain(node.bases)
            .chain(node.keywords.into_iter().map(|keyword| keyword.value))
        {
            self.visit_expr(expression);
        }
        let aliases = self.aliases.clone();
        let names = self.local_names.clone();
        let bindings = self.bindings.clone();
        let imports = self.imports.clone();
        let uncertain = self.uncertain_aliases.clone();
        self.class_outer.push(self.snapshot());
        self.local_names
            .extend(super::super::effects::conditional_names(&node.body));
        for statement in node.body {
            self.visit_stmt(statement);
        }
        self.aliases = aliases;
        self.local_names = names;
        self.bindings = bindings;
        self.imports = imports;
        self.uncertain_aliases = uncertain;
        self.class_outer.pop();
        self.aliases.remove(node.name.as_str());
        self.bindings.remove(node.name.as_str());
        self.imports.remove(node.name.as_str());
    }

    fn visit_expr_call(&mut self, node: ast::ExprCall) {
        let result = self.inspect_call(&node);
        self.save_error(result);
        match super::effects(&ast::Expr::Call(node.clone()), &self.bindings) {
            Ok(changes) if !changes.is_empty() => self.invalidate_tables(),
            Err(error) => self.error = Some(error),
            _ => {}
        }
        self.generic_visit_expr_call(node);
    }

    fn visit_expr_subscript(&mut self, node: ast::ExprSubscript) {
        let result = self.inspect_subscript(&node);
        self.save_error(result);
        self.generic_visit_expr_subscript(node);
    }

    fn visit_expr_bool_op(&mut self, node: ast::ExprBoolOp) {
        if node.op == ast::BoolOp::Or {
            for (index, expression) in node
                .values
                .iter()
                .enumerate()
                .take(node.values.len().saturating_sub(1))
            {
                let environment = match expression {
                    ast::Expr::Call(call) => {
                        self.api(&call.func).ok().flatten().is_some_and(|api| {
                            matches!(api.as_str(), "os.getenv" | "os.environ.get")
                        })
                    }
                    ast::Expr::Subscript(subscript) => {
                        self.api(&subscript.value).ok().flatten().as_deref() == Some("os.environ")
                    }
                    _ => false,
                };
                if environment {
                    let mut following = Vec::new();
                    for value in &node.values[index + 1..] {
                        following.push(self.default(Some(value)).unwrap_or_else(
                            |_| json!({"kind":"dynamic","value":self.expression(value)}),
                        ));
                    }
                    self.fallbacks.insert(expression.range().start().to_usize(),json!({"condition":if matches!(expression,ast::Expr::Subscript(_)) {"empty"}else{"missing_or_empty"},"values":following,"source":self.span(node.range)}));
                }
            }
        }
        self.generic_visit_expr_bool_op(node);
    }
}

fn scan(resolver: &mut Resolver<'_>, path: &str, source: &str) -> Result<Vec<Value>> {
    let suite =
        ast::Suite::parse(source, path).map_err(|error| anyhow::anyhow!("{path}: {error}"))?;
    let mut scanner = Scanner {
        resolver,
        path,
        source,
        bindings: Bindings::new(),
        imports: BTreeMap::new(),
        aliases: BTreeMap::new(),
        export_aliases: BTreeMap::new(),
        local_names: BTreeSet::new(),
        class_outer: Vec::new(),
        uncertain_aliases: BTreeSet::new(),
        occurrences: Vec::new(),
        fallbacks: BTreeMap::new(),
        error: None,
        depth: 0,
        nodes: 0,
    };
    for statement in suite {
        scanner.visit_stmt(statement);
    }
    if let Some(error) = scanner.error {
        return Err(error).with_context(|| format!("inventory environment reads in {path}"));
    }
    Ok(scanner.occurrences)
}

fn checked_method<'a>(
    suite: &'a [ast::Stmt],
    class: &str,
    method: &str,
) -> Result<&'a ast::StmtFunctionDef> {
    let class = suite
        .iter()
        .find_map(|stmt| match stmt {
            ast::Stmt::ClassDef(def) if def.name.as_str() == class => Some(def),
            _ => None,
        })
        .context("computed environment-name class missing")?;
    class
        .body
        .iter()
        .find_map(|stmt| match stmt {
            ast::Stmt::FunctionDef(def) if def.name.as_str() == method => Some(def),
            _ => None,
        })
        .context("computed environment-name method missing")
}

fn checked_call<'a>(
    expression: &'a ast::Expr,
    path: &str,
    arguments: usize,
) -> Result<&'a ast::ExprCall> {
    let ast::Expr::Call(call) = expression else {
        bail!("computed environment-name expression must be a call");
    };
    ensure!(
        call_path(&call.func).as_deref() == Some(path)
            && call.args.len() == arguments
            && call.keywords.is_empty(),
        "computed environment-name call changed: expected {path}"
    );
    Ok(call)
}

fn string(expression: &ast::Expr, expected: &str) -> bool {
    matches!(expression,ast::Expr::Constant(constant) if matches!(&constant.value,ast::Constant::Str(value) if value==expected))
}

fn docstring(statement: &ast::Stmt) -> bool {
    matches!(statement,ast::Stmt::Expr(expression) if matches!(&*expression.value,ast::Expr::Constant(constant) if matches!(constant.value,ast::Constant::Str(_))))
}

fn config_lookup(resolver: &Resolver<'_>) -> Result<Value> {
    let source = resolver.source(CONFIG)?;
    let suite =
        ast::Suite::parse(&source, CONFIG).map_err(|error| anyhow::anyhow!("{CONFIG}: {error}"))?;
    let function = suite
        .iter()
        .find_map(|stmt| match stmt {
            ast::Stmt::FunctionDef(def) if def.name.as_str() == "get" => Some(def),
            _ => None,
        })
        .context("config default getter missing")?;
    ensure!(
        function.body.first().is_some_and(docstring),
        "config default getter has an evaluated prefix"
    );
    ensure!(function.decorator_list.is_empty() && function.body.len()==2 && function.args.args.len()==2 && function.args.args[0].def.arg.as_str()=="key" && function.args.args[0].default.is_none() && function.args.args[1].def.arg.as_str()=="default" && function.args.args[1].default.as_deref().is_some_and(|default|matches!(default,ast::Expr::Constant(constant) if constant.value==ast::Constant::None)) && function.args.posonlyargs.is_empty() && function.args.kwonlyargs.is_empty() && function.args.vararg.is_none() && function.args.kwarg.is_none(),"config default getter structure changed");
    let ast::Stmt::Return(ret) = &function.body[1] else {
        bail!("config default getter return missing");
    };
    let Some(ast::Expr::Call(call)) = ret.value.as_deref() else {
        bail!("config default getter must return a mapping lookup");
    };
    let ast::Expr::Attribute(attr) = &*call.func else {
        bail!("config default getter mapping lookup missing");
    };
    ensure!(
        attr.attr.as_str() == "get"
            && call.args.len() == 2
            && call.keywords.is_empty()
            && call_path(&call.args[0]).as_deref() == Some("key")
            && call_path(&call.args[1]).as_deref() == Some("default"),
        "config default getter lookup changed"
    );
    checked_call(&attr.value, "load", 0)?;
    Ok(source_span(
        &source,
        CONFIG,
        resolver.tree.get(CONFIG),
        function.range,
    ))
}

fn password_family(resolver: &Resolver<'_>) -> Result<Value> {
    let source = resolver.source(KEYFILES)?;
    let suite = ast::Suite::parse(&source, KEYFILES)
        .map_err(|error| anyhow::anyhow!("{KEYFILES}: {error}"))?;
    let method = checked_method(&suite, "Keyfile", "env_var_name")?;
    ensure!(
        method.decorator_list.is_empty()
            && method.body.len() == 2
            && method.args.args.len() == 1
            && method.args.args[0].def.arg.as_str() == "self"
            && method.args.args[0].default.is_none()
            && method.args.posonlyargs.is_empty()
            && method.args.kwonlyargs.is_empty()
            && method.args.vararg.is_none()
            && method.args.kwarg.is_none(),
        "computed environment-name method structure changed"
    );
    let ast::Stmt::Assign(normalized) = &method.body[0] else {
        bail!("computed environment-name normalization missing");
    };
    ensure!(
        normalized.targets.len() == 1
            && call_path(&normalized.targets[0]).as_deref() == Some("normalized"),
        "computed environment-name normalization target changed"
    );
    let ast::Expr::Call(dot) = &*normalized.value else {
        bail!("computed environment-name normalization must be a call");
    };
    let ast::Expr::Attribute(replace) = &*dot.func else {
        bail!("computed environment-name dot replacement missing");
    };
    ensure!(
        replace.attr.as_str() == "replace"
            && dot.args.len() == 2
            && dot.keywords.is_empty()
            && string(&dot.args[0], ".")
            && string(&dot.args[1], "_"),
        "computed environment-name dot replacement changed"
    );
    let separator = checked_call(&replace.value, "self.path.replace", 2)?;
    ensure!(
        call_path(&separator.args[0]).as_deref() == Some("os.sep")
            && string(&separator.args[1], "_"),
        "computed environment-name path separator replacement changed"
    );
    let ast::Stmt::Return(ret) = &method.body[1] else {
        bail!("computed environment-name return missing");
    };
    let Some(ast::Expr::JoinedStr(joined)) = ret.value.as_deref() else {
        bail!("computed environment-name must use the checked f-string");
    };
    ensure!(
        joined.values.len() == 2 && string(&joined.values[0], "BT_PW_"),
        "computed environment-name prefix changed"
    );
    let ast::Expr::FormattedValue(value) = &joined.values[1] else {
        bail!("computed environment-name suffix is not formatted");
    };
    ensure!(
        value.conversion == ast::ConversionFlag::None && value.format_spec.is_none(),
        "computed environment-name formatting changed"
    );
    checked_call(&value.value, "normalized.upper", 0)?;

    // The SDK's family read passes through the native binding, not os.getenv.
    let reader = suite
        .iter()
        .find_map(|stmt| match stmt {
            ast::Stmt::FunctionDef(def) if def.name.as_str() == "resolve_key_password" => Some(def),
            _ => None,
        })
        .context("native environment-password reader missing")?;
    ensure!(
        reader.decorator_list.is_empty()
            && reader.body.len() == 5
            && docstring(&reader.body[0])
            && reader.args.args.len() == 1
            && reader.args.args[0].def.arg.as_str() == "password"
            && reader.args.posonlyargs.is_empty()
            && reader.args.vararg.is_none()
            && reader.args.kwarg.is_none()
            && reader
                .args
                .kwonlyargs
                .iter()
                .filter(|arg| arg.def.arg.as_str() == "env_var_name")
                .count()
                == 1,
        "native environment-password reader structure changed"
    );
    let ast::Stmt::If(explicit) = &reader.body[1] else {
        bail!("explicit password branch missing");
    };
    ensure!(
        call_path(&explicit.test).as_deref() == Some("password")
            && explicit.body.len() == 1
            && explicit.orelse.is_empty(),
        "explicit password guard changed"
    );
    let ast::Stmt::Return(ret) = &explicit.body[0] else {
        bail!("explicit password return missing");
    };
    ensure!(
        ret.value.as_deref().and_then(call_path).as_deref() == Some("password"),
        "explicit password return changed"
    );
    let ast::Stmt::If(branch) = reader
        .body
        .get(2)
        .context("native environment-password branch missing")?
    else {
        bail!("native environment-password reader structure changed");
    };
    ensure!(
        call_path(&branch.test).as_deref() == Some("env_var_name"),
        "native environment-password guard changed"
    );
    let Some(ast::Stmt::Assign(assign)) = branch.body.first() else {
        bail!("native environment-password read missing");
    };
    let read = checked_call(&assign.value, "get_password_from_environment", 1)?;
    ensure!(
        assign.targets.len() == 1
            && call_path(&assign.targets[0]).as_deref() == Some("env_password")
            && call_path(&read.args[0]).as_deref() == Some("env_var_name")
            && branch.body.len() == 2
            && branch.orelse.is_empty(),
        "native environment-password key changed"
    );
    let ast::Stmt::If(nonempty) = &branch.body[1] else {
        bail!("native environment-password nonempty guard missing");
    };
    ensure!(
        call_path(&nonempty.test).as_deref() == Some("env_password")
            && nonempty.body.len() == 1
            && nonempty.orelse.is_empty(),
        "native environment-password empty behavior changed"
    );
    let ast::Stmt::Return(ret) = &nonempty.body[0] else {
        bail!("native environment-password nonempty return missing");
    };
    ensure!(
        ret.value.as_deref().and_then(call_path).as_deref() == Some("env_password"),
        "native environment-password nonempty return changed"
    );
    let keypair = checked_method(&suite, "Keyfile", "get_keypair")?;
    let mut linked = false;
    for stmt in &keypair.body {
        if let ast::Stmt::If(branch) = stmt {
            for stmt in &branch.body {
                if let ast::Stmt::Assign(assign) = stmt
                    && let ast::Expr::Call(call) = &*assign.value
                    && call_path(&call.func).as_deref() == Some("resolve_key_password")
                {
                    for keyword in &call.keywords {
                        if keyword
                            .arg
                            .as_ref()
                            .is_some_and(|name| name == "env_var_name")
                        {
                            checked_call(&keyword.value, "self.env_var_name", 0)?;
                            linked = true;
                        }
                    }
                }
            }
        }
    }
    ensure!(
        linked,
        "computed environment-name no longer reaches the native read"
    );
    Ok(
        json!({"name":"BT_PW_*","api":"bittensor.sp_core.get_password_from_environment","default":{"kind":"missing"},"missing_value":"returns_none","empty_value":"ignored_by_truthiness_guard","truthy_fallback":null,"source":source_span(&source,KEYFILES,resolver.tree.get(KEYFILES),read.range),"family":{"prefix":"BT_PW_","input":"self.path","transforms":[{"replace":"os.sep","with":"_"},{"replace":".","with":"_"},"upper"],"definition":source_span(&source,KEYFILES,resolver.tree.get(KEYFILES),method.range),"caller":source_span(&source,KEYFILES,resolver.tree.get(KEYFILES),keypair.range)}}),
    )
}

fn parity_comparison(root: &Path, names: &BTreeSet<String>) -> Result<Value> {
    let path = root.join("parity/env.toml");
    let text=fs::read_to_string(&path).with_context(||"parity/env.toml is required; map every extracted environment name to an existing consumer or a reasoned divergence")?;
    parity_map(root, names, Some(&text))
}

fn consumer_exists(path: &Path, consumer: &str) -> Result<bool> {
    use syn::visit::Visit;
    #[derive(Default)]
    struct Functions(BTreeSet<String>);
    impl<'ast> Visit<'ast> for Functions {
        fn visit_item_fn(&mut self, function: &'ast syn::ItemFn) {
            if matches!(function.vis, syn::Visibility::Public(_)) {
                self.0.insert(function.sig.ident.to_string());
            }
        }
        fn visit_item_impl(&mut self, implementation: &'ast syn::ItemImpl) {
            if let syn::Type::Path(ty) = &*implementation.self_ty
                && let Some(container) = ty.path.segments.last()
            {
                for item in &implementation.items {
                    if let syn::ImplItem::Fn(function) = item
                        && matches!(function.vis, syn::Visibility::Public(_))
                    {
                        self.0
                            .insert(format!("{}::{}", container.ident, function.sig.ident));
                    }
                }
            }
        }
    }
    let source = fs::read_to_string(path)?;
    ensure!(
        source.len() <= super::super::MAX_SOURCE_BYTES,
        "environment consumer source exceeds byte bound"
    );
    let syntax = syn::parse_file(&source)
        .with_context(|| format!("parse environment consumer source {}", path.display()))?;
    let mut functions = Functions::default();
    functions.visit_file(&syntax);
    Ok(functions
        .0
        .iter()
        .any(|function| consumer.ends_with(&format!("::{function}"))))
}

fn parity_map(root: &Path, names: &BTreeSet<String>, text: Option<&str>) -> Result<Value> {
    let text = text.context("parity/env.toml is absent; environment coverage must be explicit")?;
    let map = text
        .parse::<DocumentMut>()
        .context("parse parity/env.toml")?;
    ensure!(
        map.get("schema").and_then(|item| item.as_integer()) == Some(1),
        "parity/env.toml requires schema = 1"
    );
    ensure!(
        !(map.get("env").is_some() && map.get("variables").is_some()),
        "environment parity map must use only one of [env] or [variables]"
    );
    let env = map
        .get("env")
        .or_else(|| map.get("variables"))
        .and_then(|item| item.as_table_like())
        .context("parity/env.toml requires [env.NAME] or [variables.NAME] decisions")?;
    let mut mapped = BTreeSet::new();
    let mut consumers = 0usize;
    let mut divergences = 0usize;
    let mut pending = 0usize;
    for (name, item) in env.iter() {
        let row = item
            .as_table_like()
            .with_context(|| format!("environment parity row {name} must be a table"))?;
        let field = |key: &str| {
            row.get(key)
                .and_then(|item| item.as_str())
                .filter(|value| !value.trim().is_empty())
        };
        let consumer = field("consumer");
        let divergence = field("divergence");
        ensure!(
            consumer.is_some() != divergence.is_some(),
            "environment {name} must name exactly one existing consumer or divergence"
        );
        let evidence = if consumer.is_some() {
            consumers += 1;
            ensure!(
                field("notes").is_some(),
                "environment consumer {name} requires notes describing its semantics"
            );
            field("rust_path").with_context(|| {
                format!("environment consumer {name} requires a Rust source path")
            })?
        } else {
            divergences += 1;
            let class = field("class")
                .with_context(|| format!("environment divergence {name} requires a class"))?;
            ensure!(
                matches!(class, "pending" | "presentation"),
                "environment divergence {name} has unsupported class {class}"
            );
            pending += usize::from(class == "pending");
            for key in [
                "python_behavior",
                "rtao_behavior",
                "rationale",
                "resolution",
            ] {
                ensure!(
                    field(key).is_some(),
                    "environment divergence {name} requires {key}"
                );
            }
            field("evidence")
                .with_context(|| format!("environment divergence {name} requires evidence"))?
        };
        let evidence = Path::new(evidence);
        ensure!(
            evidence.starts_with("crates")
                && evidence
                    .components()
                    .all(|component| matches!(component, std::path::Component::Normal(_)))
                && evidence
                    .extension()
                    .is_some_and(|extension| extension == "rs")
                && root.join(evidence).is_file(),
            "environment {name} evidence must name an existing repository Rust source"
        );
        if let Some(consumer) = consumer {
            let crate_name = evidence
                .components()
                .nth(1)
                .context("environment source has no crate directory")?
                .as_os_str()
                .to_str()
                .context("environment crate path is not UTF-8")?
                .replace('-', "_");
            ensure!(
                consumer.starts_with(&format!("{crate_name}::"))
                    && consumer_exists(&root.join(evidence), consumer)?,
                "environment {name} consumer {consumer} is not a public function/method in its declared Rust source"
            );
        }
        mapped.insert(name.to_owned());
    }
    let unmapped = names.difference(&mapped).collect::<Vec<_>>();
    let vanished = mapped.difference(names).collect::<Vec<_>>();
    ensure!(
        unmapped.is_empty() && vanished.is_empty(),
        "environment inventory differs from parity/env.toml: unmapped={unmapped:?}; vanished={vanished:?}"
    );
    ensure!(!mapped.is_empty(), "environment parity map is empty");
    Ok(
        json!({"path":"parity/env.toml","status":"matches","consumers":consumers,"divergences":divergences,"pending":pending}),
    )
}

pub(super) fn outputs(
    root: &Path,
    pins: &DocumentMut,
    _tables: &Value,
) -> Result<Vec<(&'static str, Vec<u8>)>> {
    let tree = Resolver::new(root)?.tree;
    let paths = tree
        .keys()
        .filter(|path| path.starts_with(PY) && path.ends_with(".py"))
        .cloned()
        .collect::<Vec<_>>();
    let mut occurrences = Vec::new();
    for path in &paths {
        let mut resolver = Resolver::new(root)?;
        let source = resolver.source(path)?;
        occurrences.extend(scan(&mut resolver, path, &source)?);
        ensure!(
            occurrences.len() < super::super::MAX_CONTAINER_ITEMS,
            "package environment occurrences exceed inventory bound"
        );
    }
    occurrences.push(password_family(&Resolver::new(root)?)?);
    occurrences.sort_by_key(|row| {
        (
            row["source"]["path"].as_str().unwrap_or("").to_owned(),
            row["source"]["start_byte"].as_u64().unwrap_or(0),
            row["name"].as_str().unwrap_or("").to_owned(),
        )
    });
    let names = occurrences
        .iter()
        .map(|row| {
            row["name"]
                .as_str()
                .context("environment occurrence has no name")
                .map(str::to_owned)
        })
        .collect::<Result<BTreeSet<_>>>()?;
    let mut inventory = BTreeMap::<String, Vec<usize>>::new();
    for (index, row) in occurrences.iter().enumerate() {
        inventory
            .entry(
                row["name"]
                    .as_str()
                    .context("environment occurrence has no name")?
                    .to_owned(),
            )
            .or_default()
            .push(index);
    }
    let mut document = json!({"format":1,"provenance":{"tag":pins["parity"]["tag"].as_str().context("parity tag missing")?,"commit":pins["parity"]["commit"].as_str().context("parity commit missing")?},"scope":{"root":PY,"authenticated_modules":paths.len()},"inventory":inventory,"occurrences":occurrences,"parity":parity_comparison(root,&names)?});
    document.sort_all_objects();
    Ok(vec![(
        "vendor/upstream/extracted/env_vars.json",
        pretty(&document)?,
    )])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> Result<std::path::PathBuf> {
        Ok(Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .context("workspace parent missing")?
            .to_path_buf())
    }

    fn fixture(source: &str) -> Result<Vec<Value>> {
        let root = root()?;
        let mut resolver = Resolver::new(&root)?;
        resolver.sources.insert("fixture.py".into(), source.into());
        scan(&mut resolver, "fixture.py", source)
    }

    #[test]
    fn aliases_names_defaults_and_subscript_reads() -> Result<()> {
        let rows = fixture(
            "import os as operating\nfrom os import getenv as read\nfrom os import environ as environment\nfrom typer import Option as option\nENV_VAR = 'SDK_LOG'\nA = operating.getenv('A')\nB = read('B', None)\nC = environment.get(ENV_VAR, '')\nD = environment['D']\nE = option(False, envvar=['E', 'F'])\n",
        )?;
        assert_eq!(
            rows.iter()
                .map(|row| row["name"].as_str().context("occurrence name missing"))
                .collect::<Result<Vec<_>>>()?,
            ["A", "B", "SDK_LOG", "D", "E", "F"]
        );
        assert_eq!(rows[0]["default"]["kind"], "missing");
        assert_eq!(rows[1]["default"]["value"], Value::Null);
        assert_eq!(rows[2]["default"]["value"], "");
        assert_eq!(rows[3]["missing_value"], "raises_key_error");
        assert_eq!(rows[4]["empty_value"], "treated_as_unset_by_click");
        assert!(rows.iter().all(|row| {
            row["source"]["start_byte"]
                .as_u64()
                .zip(row["source"]["end_byte"].as_u64())
                .is_some_and(|(start, end)| start < end)
        }));
        Ok(())
    }

    #[test]
    fn shadowing_and_nested_reads_are_respected() -> Result<()> {
        let rows = fixture(
            "import os\ndef local(os):\n    return os.getenv('SHADOW')\ndef reader():\n    from os import getenv as read\n    return (lambda: read('LIVE'))()\nA = os.getenv('OUTER') or '/tmp/default'\n",
        )?;
        assert_eq!(
            rows.iter()
                .map(|row| row["name"].as_str().context("occurrence name missing"))
                .collect::<Result<Vec<_>>>()?,
            ["LIVE", "OUTER"]
        );
        assert_eq!(rows[1]["truthy_fallback"]["condition"], "missing_or_empty");
        assert_eq!(
            rows[1]["truthy_fallback"]["values"][0]["value"],
            "/tmp/default"
        );
        assert!(
            fixture("import os\nKEY = 'GLOBAL'\ndef reader(KEY):\n    return os.getenv(KEY)\n")
                .is_err()
        );
        assert!(fixture("import os\nA = os.getenv(prefix + suffix)\n").is_err());
        Ok(())
    }

    #[test]
    fn argument_keyword_comprehension_with_and_match_reads_are_visited() -> Result<()> {
        let rows = fixture(
            "import os\nimport typer\ndef f(flag=typer.Option(None, envvar='ARG')):\n    send(value=os.getenv('KW'))\n    with acquire(os.getenv('WITH')):\n        pass\n    match value:\n        case [] if os.getenv('GUARD'):\n            return os.getenv('BODY')\nV = [os.getenv('ELT') for value in (os.getenv('ITER'),) if os.getenv('FILTER')]\nH = [os.getenv('SHADOW') for os in objects]\nclass C:\n    os = object()\n    def method(self):\n        return os.getenv('METHOD_GLOBAL')\n",
        )?;
        let names = rows
            .iter()
            .map(|row| row["name"].as_str().context("occurrence name missing"))
            .collect::<Result<BTreeSet<_>>>()?;
        assert_eq!(
            names,
            BTreeSet::from([
                "ARG",
                "KW",
                "WITH",
                "GUARD",
                "BODY",
                "ITER",
                "FILTER",
                "ELT",
                "METHOD_GLOBAL"
            ])
        );
        assert!(
            fixture("import os\nKEY: str = 'ANNOTATED'\nV = os.getenv(KEY)\n")?
                .iter()
                .any(|row| row["name"] == "ANNOTATED")
        );
        let changed = fixture("import os\nos: object = object()\nV = os.getenv('SHADOW')\n")?;
        assert!(changed.is_empty());
        Ok(())
    }

    #[test]
    fn rebound_names_and_imported_namespace_aliases_are_checked() -> Result<()> {
        for source in [
            "import os\nKEY='BEFORE'\nKEY += '_AFTER'\nV=os.getenv(KEY)\n",
            "import os\nKEY='BEFORE'\nfor KEY in names:\n    V=os.getenv(KEY)\n",
            "import os\nKEY='BEFORE'\ndef KEY():\n    pass\nV=os.getenv(KEY)\n",
            "import os\nKEYS={'a':'A'}\nKEYS['a']='B'\nV=os.getenv(KEYS['a'])\n",
            "import typer\nKEYS=['A']\nKEYS.append('B')\nV=typer.Option(None,envvar=KEYS)\n",
            "import os\nif flag:\n    os=object()\nV=os.getenv('MAYBE')\n",
        ] {
            assert!(fixture(source).is_err());
        }
        assert_eq!(
            fixture("import os\nKEYS={'a':'A'}\nV=os.getenv(KEYS['a'])\n")?[0]["name"],
            "A"
        );
        let root = root()?;
        let path = "sdk/python/bittensor/env_fixture.py";
        let source = "from .env_alias_fixture import read as grab\nV=grab('IMPORTED')\n";
        let mut resolver = Resolver::new(&root)?;
        resolver.sources.insert(path.into(), source.into());
        resolver.sources.insert(
            "sdk/python/bittensor/env_alias_fixture.py".into(),
            "import os as operating\nread=operating.getenv\n".into(),
        );
        let rows = scan(&mut resolver, path, source)?;
        assert_eq!(rows[0]["name"], "IMPORTED");
        assert_eq!(rows[0]["api"], "os.getenv");
        let rows = fixture(
            "import os\nKEY='GLOBAL'\nclass C:\n    KEY='CLASS'\n    def method(self):\n        return os.getenv(KEY)\nV=os.environ['X'] or 'fallback'\n",
        )?;
        assert_eq!(rows[0]["name"], "GLOBAL");
        assert_eq!(rows[1]["truthy_fallback"]["condition"], "empty");
        let rows = fixture(
            "import os\ntry:\n    pass\nexcept Exception as os:\n    V=os.getenv('SHADOW')\nmatch value:\n    case os:\n        V=os.getenv('SHADOW')\n",
        )?;
        assert!(rows.is_empty());
        let rows = fixture(
            "from bittensor_core import get_password_from_environment as read\nV=read('NATIVE_STATIC')\n",
        )?;
        assert_eq!(rows[0]["name"], "NATIVE_STATIC");
        Ok(())
    }

    #[test]
    fn password_family_refuses_changed_normalization() -> Result<()> {
        let root = root()?;
        let mut resolver = Resolver::new(&root)?;
        let source = resolver.source(KEYFILES)?;
        assert_eq!(password_family(&resolver)?["name"], "BT_PW_*");
        for changed in [
            source.replace("normalized.upper()", "normalized.lower()"),
            source.replace("replace(\".\", \"_\")", "replace(\".\", \"-\")"),
            source.replace("env_var_name=self.env_var_name()", "env_var_name='STATIC'"),
            source.replace("if env_password:", "if env_password is not None:"),
            source.replace(
                "if password:\n        return password",
                "env_var_name = 'STATIC'",
            ),
        ] {
            resolver.sources.insert(KEYFILES.into(), changed);
            assert!(password_family(&resolver).is_err());
        }
        Ok(())
    }

    #[test]
    fn config_lookup_refuses_changed_fallback_semantics() -> Result<()> {
        let root = root()?;
        let mut resolver = Resolver::new(&root)?;
        let source = resolver.source(CONFIG)?;
        assert_eq!(config_lookup(&resolver)?["path"], CONFIG);
        resolver.sources.insert(
            CONFIG.into(),
            source.replace(
                "return load().get(key, default)",
                "return load().get(key) or default",
            ),
        );
        assert!(config_lookup(&resolver).is_err());
        resolver.sources.insert(CONFIG.into(),source.replace("    \"\"\"The stored value for ``key``, or ``default`` — used to seed CLI option defaults.\"\"\"","    default = 'OVERRIDE'"));
        assert!(config_lookup(&resolver).is_err());
        Ok(())
    }

    #[test]
    fn inventory_scans_the_authenticated_package() -> Result<()> {
        let root = root()?;
        let pins = fs::read_to_string(root.join("upstream.toml"))?.parse::<DocumentMut>()?;
        let generated = outputs(&root, &pins, &Value::Null)?;
        let inventory: Value = serde_json::from_slice(&generated[0].1)?;
        let names = inventory["inventory"]
            .as_object()
            .context("inventory map missing")?;
        assert_eq!(names.len(), 29);
        assert_eq!(inventory["parity"]["status"], "matches");
        for name in [
            "BITTENSOR_LOG",
            "BT_PW_*",
            "BT_NETWORK",
            "BT_WALLET_PATH",
            "GH_TOKEN",
            "GITHUB_TOKEN",
        ] {
            assert!(
                names.contains_key(name),
                "{name} absent from environment inventory"
            );
        }
        let rows = inventory["occurrences"]
            .as_array()
            .context("occurrences missing")?;
        let network = rows
            .iter()
            .find(|row| row["name"] == "BT_NETWORK")
            .context("network occurrence missing")?;
        assert_eq!(network["default"]["kind"], "config_lookup");
        assert_eq!(network["default"]["fallback"]["value"], "finney");
        assert!(rows.iter().all(|row| {
            row["source"]["blob"]
                .as_str()
                .is_some_and(|blob| blob.len() == 40)
        }));
        Ok(())
    }

    #[test]
    fn environment_map_rejects_absent_changed_and_unjustified_decisions() -> Result<()> {
        let root = root()?;
        let names = BTreeSet::from(["EXISTING".to_owned()]);
        let consumer = "schema=1\n[env.EXISTING]\nconsumer='rtao_client::Network::resolve'\nrust_path='crates/rtao-client/src/network.rs'\nnotes='Explicit local endpoint resolver, with empty override fallback.'\n";
        assert_eq!(parity_map(&root, &names, Some(consumer))?["consumers"], 1);
        assert!(parity_map(&root, &names, None).is_err());
        let added = BTreeSet::from(["EXISTING".to_owned(), "NEW".to_owned()]);
        assert!(
            parity_map(&root, &added, Some(consumer))
                .err()
                .context("new variable accepted")?
                .to_string()
                .contains("unmapped=[\"NEW\"]")
        );
        let renamed = BTreeSet::from(["RENAMED".to_owned()]);
        let error = parity_map(&root, &renamed, Some(consumer))
            .err()
            .context("vanished variable accepted")?
            .to_string();
        assert!(
            error.contains("unmapped=[\"RENAMED\"]") && error.contains("vanished=[\"EXISTING\"]")
        );
        for changed in [
            consumer.replace("Network::resolve", "Network::not_implemented"),
            consumer.replace(
                "crates/rtao-client/src/network.rs",
                "crates/rtao-client/src/not_present.rs",
            ),
            format!("{consumer}divergence='ENV-AMBIGUOUS'\n"),
        ] {
            assert!(parity_map(&root, &names, Some(&changed)).is_err());
        }
        let divergence = "schema=1\n[env.EXISTING]\ndivergence='ENV-PENDING-TEST'\nclass='pending'\npython_behavior='Reads the named global option.'\nrtao_behavior='No process reader exists yet.'\nrationale='Assigned to the planned CLI milestone.'\nresolution='Replace this row when the reader lands.'\nevidence='crates/rtao-cli/src/main.rs'\n";
        assert_eq!(parity_map(&root, &names, Some(divergence))?["pending"], 1);
        assert!(
            parity_map(
                &root,
                &names,
                Some(
                    &divergence.replace("rationale='Assigned to the planned CLI milestone.'\n", "")
                )
            )
            .is_err()
        );
        assert_eq!(
            parity_map(
                &root,
                &names,
                Some(&consumer.replace("[env.EXISTING]", "[variables.EXISTING]"))
            )?["consumers"],
            1
        );
        Ok(())
    }
}
