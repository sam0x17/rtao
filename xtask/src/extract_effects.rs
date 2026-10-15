//! Borrowed traversal of expressions evaluated while Python defines tables.

use std::collections::BTreeSet;

use anyhow::{Result, ensure};
use rustpython_parser::ast;

use super::MAX_DEPTH;

pub(super) fn walk(
    expr: &ast::Expr,
    depth: usize,
    visit: &mut impl FnMut(&ast::Expr),
) -> Result<()> {
    ensure!(depth < MAX_DEPTH, "effect expression exceeds depth limit");
    visit(expr);
    let next = depth + 1;
    match expr {
        ast::Expr::BoolOp(e) => sequence(&e.values, next, visit)?,
        ast::Expr::NamedExpr(e) => {
            walk(&e.target, next, visit)?;
            walk(&e.value, next, visit)?;
        }
        ast::Expr::BinOp(e) => {
            walk(&e.left, next, visit)?;
            walk(&e.right, next, visit)?;
        }
        ast::Expr::UnaryOp(e) => walk(&e.operand, next, visit)?,
        ast::Expr::Lambda(e) => {
            for argument in e
                .args
                .posonlyargs
                .iter()
                .chain(&e.args.args)
                .chain(&e.args.kwonlyargs)
            {
                if let Some(default) = &argument.default {
                    walk(default, next, visit)?;
                }
            }
        }
        ast::Expr::IfExp(e) => {
            walk(&e.test, next, visit)?;
            walk(&e.body, next, visit)?;
            walk(&e.orelse, next, visit)?;
        }
        ast::Expr::Dict(e) => {
            for key in e.keys.iter().flatten() {
                walk(key, next, visit)?;
            }
            sequence(&e.values, next, visit)?;
        }
        ast::Expr::Set(e) => sequence(&e.elts, next, visit)?,
        ast::Expr::ListComp(e) => {
            walk(&e.elt, next, visit)?;
            comprehensions(&e.generators, next, visit)?;
        }
        ast::Expr::SetComp(e) => {
            walk(&e.elt, next, visit)?;
            comprehensions(&e.generators, next, visit)?;
        }
        ast::Expr::DictComp(e) => {
            walk(&e.key, next, visit)?;
            walk(&e.value, next, visit)?;
            comprehensions(&e.generators, next, visit)?;
        }
        ast::Expr::GeneratorExp(e) => {
            walk(&e.elt, next, visit)?;
            comprehensions(&e.generators, next, visit)?;
        }
        ast::Expr::Await(e) => walk(&e.value, next, visit)?,
        ast::Expr::Yield(e) => {
            if let Some(value) = &e.value {
                walk(value, next, visit)?;
            }
        }
        ast::Expr::YieldFrom(e) => walk(&e.value, next, visit)?,
        ast::Expr::Compare(e) => {
            walk(&e.left, next, visit)?;
            sequence(&e.comparators, next, visit)?;
        }
        ast::Expr::Call(e) => {
            walk(&e.func, next, visit)?;
            sequence(&e.args, next, visit)?;
            for keyword in &e.keywords {
                walk(&keyword.value, next, visit)?;
            }
        }
        ast::Expr::FormattedValue(e) => {
            walk(&e.value, next, visit)?;
            if let Some(spec) = &e.format_spec {
                walk(spec, next, visit)?;
            }
        }
        ast::Expr::JoinedStr(e) => sequence(&e.values, next, visit)?,
        ast::Expr::Constant(_) | ast::Expr::Name(_) => {}
        ast::Expr::Attribute(e) => walk(&e.value, next, visit)?,
        ast::Expr::Subscript(e) => {
            walk(&e.value, next, visit)?;
            walk(&e.slice, next, visit)?;
        }
        ast::Expr::Starred(e) => walk(&e.value, next, visit)?,
        ast::Expr::List(e) => sequence(&e.elts, next, visit)?,
        ast::Expr::Tuple(e) => sequence(&e.elts, next, visit)?,
        ast::Expr::Slice(e) => {
            for item in [&e.lower, &e.upper, &e.step].into_iter().flatten() {
                walk(item, next, visit)?;
            }
        }
    }
    Ok(())
}

fn sequence(exprs: &[ast::Expr], depth: usize, visit: &mut impl FnMut(&ast::Expr)) -> Result<()> {
    for expr in exprs {
        walk(expr, depth, visit)?;
    }
    Ok(())
}

fn comprehensions(
    generators: &[ast::Comprehension],
    depth: usize,
    visit: &mut impl FnMut(&ast::Expr),
) -> Result<()> {
    for generator in generators {
        walk(&generator.iter, depth, visit)?;
        sequence(&generator.ifs, depth, visit)?;
    }
    Ok(())
}

pub(super) fn names(expr: &ast::Expr) -> Result<BTreeSet<String>> {
    let mut names = BTreeSet::new();
    walk(expr, 0, &mut |expr| {
        if let ast::Expr::Name(name) = expr
            && name.ctx == ast::ExprContext::Load
        {
            names.insert(name.id.to_string());
        }
    })?;
    Ok(names)
}

pub(super) fn conditional_names(body: &[ast::Stmt]) -> BTreeSet<String> {
    use rustpython_ast::Visitor;
    #[derive(Default)]
    struct Names(BTreeSet<String>);
    impl Visitor for Names {
        fn visit_withitem(&mut self, node: ast::WithItem) {
            if let Some(vars) = node.optional_vars {
                self.visit_expr(*vars);
            }
        }
        fn visit_excepthandler_except_handler(&mut self, node: ast::ExceptHandlerExceptHandler) {
            if let Some(name) = node.name {
                self.0.insert(name.to_string());
            }
            for stmt in node.body {
                self.visit_stmt(stmt);
            }
        }
        fn visit_expr_name(&mut self, node: ast::ExprName) {
            if node.ctx != ast::ExprContext::Load {
                self.0.insert(node.id.to_string());
            }
        }
        fn visit_stmt_function_def(&mut self, node: ast::StmtFunctionDef) {
            self.0.insert(node.name.to_string());
        }
        fn visit_stmt_async_function_def(&mut self, node: ast::StmtAsyncFunctionDef) {
            self.0.insert(node.name.to_string());
        }
        fn visit_stmt_class_def(&mut self, node: ast::StmtClassDef) {
            self.0.insert(node.name.to_string());
        }
        fn visit_stmt_import(&mut self, node: ast::StmtImport) {
            for alias in node.names {
                self.0.insert(alias.asname.map_or_else(
                    || alias.name.split('.').next().unwrap_or("").to_owned(),
                    |name| name.to_string(),
                ));
            }
        }
        fn visit_stmt_import_from(&mut self, node: ast::StmtImportFrom) {
            for alias in node.names {
                self.0
                    .insert(alias.asname.unwrap_or(alias.name).to_string());
            }
        }
        fn visit_pattern_match_as(&mut self, node: ast::PatternMatchAs) {
            if let Some(name) = node.name {
                self.0.insert(name.to_string());
            }
            if let Some(pattern) = node.pattern {
                self.visit_pattern(*pattern);
            }
        }
        fn visit_pattern_match_star(&mut self, node: ast::PatternMatchStar) {
            if let Some(name) = node.name {
                self.0.insert(name.to_string());
            }
        }
        fn visit_pattern_match_mapping(&mut self, node: ast::PatternMatchMapping) {
            if let Some(name) = node.rest.clone() {
                self.0.insert(name.to_string());
            }
            self.generic_visit_pattern_match_mapping(node);
        }
    }
    let mut names = Names::default();
    for stmt in body {
        names.visit_stmt(stmt.clone());
    }
    names.0
}

pub(super) fn all_expressions(
    body: &[ast::Stmt],
    visit: &mut impl FnMut(&ast::Expr),
) -> Result<()> {
    use rustpython_ast::Visitor;
    struct Expressions<'a, F>(&'a mut F, Option<anyhow::Error>);
    impl<F: FnMut(&ast::Expr)> Visitor for Expressions<'_, F> {
        fn visit_expr(&mut self, node: ast::Expr) {
            if self.1.is_none()
                && let Err(error) = walk(&node, 0, self.0)
            {
                self.1 = Some(error);
            }
        }
        fn visit_keyword(&mut self, node: ast::Keyword) {
            self.visit_expr(node.value);
        }
        fn visit_withitem(&mut self, node: ast::WithItem) {
            self.visit_expr(node.context_expr);
        }
        fn visit_arguments(&mut self, node: ast::Arguments) {
            for argument in node
                .posonlyargs
                .into_iter()
                .chain(node.args)
                .chain(node.kwonlyargs)
            {
                if let Some(default) = argument.default {
                    self.visit_expr(*default);
                }
            }
        }
    }
    let mut expressions = Expressions(visit, None);
    for stmt in body {
        expressions.visit_stmt(stmt.clone());
    }
    if let Some(error) = expressions.1 {
        return Err(error);
    }
    Ok(())
}
