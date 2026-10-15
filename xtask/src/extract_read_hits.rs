//! Authenticated, module-local transitive descriptor coverage for catalog reads.

use super::{Resolver, pretty};
use anyhow::{Context, Result, bail, ensure};
use rustpython_parser::{
    Parse,
    ast::{self, Ranged},
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};
use toml_edit::DocumentMut;

type Hit = (String, String, String);

fn chain(expression: &ast::Expr) -> Option<Vec<String>> {
    match expression {
        ast::Expr::Name(name) => Some(vec![name.id.to_string()]),
        ast::Expr::Attribute(attribute) => {
            let mut names = chain(&attribute.value)?;
            names.push(attribute.attr.to_string());
            Some(names)
        }
        _ => None,
    }
}
fn text(expression: &ast::Expr) -> Option<&str> {
    match expression {
        ast::Expr::Constant(c) => match &c.value {
            ast::Constant::Str(s) => Some(s),
            _ => None,
        },
        _ => None,
    }
}

struct Module {
    path: String,
    blob: String,
    source: String,
    suite: ast::Suite,
    aliases: BTreeMap<String, String>,
    literals: BTreeMap<String, Hit>,
    functions: BTreeMap<String, Vec<ast::Stmt>>,
}
impl Module {
    fn load(resolver: &Resolver<'_>, path: &str) -> Result<Self> {
        let source = resolver.source(path)?;
        let suite = ast::Suite::parse(&source, path)?;
        let mut aliases = BTreeMap::new();
        let mut constructors = BTreeMap::new();
        let mut functions = BTreeMap::new();
        for stmt in &suite {
            match stmt {
                ast::Stmt::ImportFrom(import) => {
                    let Some(module) = import.module.as_ref().map(|m| m.as_str()) else {
                        continue;
                    };
                    for alias in &import.names {
                        let local = alias.asname.as_ref().unwrap_or(&alias.name).to_string();
                        let name = alias.name.as_str();
                        if module == "_generated"
                            && ["storage", "runtime_apis", "constants"].contains(&name)
                        {
                            ensure!(
                                aliases.insert(local.clone(), name.into()).is_none(),
                                "descriptor alias rebound"
                            );
                        }
                        let kind = match (module, name) {
                            ("_generated.storage", "Item") => Some("storage"),
                            ("_generated.runtime_apis", "Method") => Some("runtime"),
                            ("_generated.constants", "Constant") => Some("constant"),
                            _ => None,
                        };
                        if let Some(kind) = kind {
                            constructors.insert(local, kind.to_owned());
                        }
                    }
                }
                ast::Stmt::FunctionDef(function) => {
                    ensure!(
                        functions
                            .insert(function.name.to_string(), function.body.clone())
                            .is_none(),
                        "duplicate helper"
                    );
                }
                ast::Stmt::AsyncFunctionDef(function) => {
                    ensure!(
                        functions
                            .insert(function.name.to_string(), function.body.clone())
                            .is_none(),
                        "duplicate helper"
                    );
                }
                _ => {}
            }
        }
        let mut literals = BTreeMap::new();
        for stmt in &suite {
            let (targets, value) = match stmt {
                ast::Stmt::Assign(assign) => (&assign.targets, assign.value.as_ref()),
                _ => continue,
            };
            let ast::Expr::Call(call) = value else {
                continue;
            };
            let Some(callee) = chain(&call.func)
                .filter(|p| p.len() == 1)
                .and_then(|p| p.into_iter().next())
            else {
                continue;
            };
            let Some(kind) = constructors.get(&callee) else {
                continue;
            };
            ensure!(call.args.len() >= 2, "descriptor constructor changed");
            let pallet = text(&call.args[0]).context("descriptor container must be literal")?;
            let item = text(&call.args[1]).context("descriptor name must be literal")?;
            for target in targets {
                let ast::Expr::Name(name) = target else {
                    bail!("descriptor target must be name")
                };
                ensure!(
                    literals
                        .insert(
                            name.id.to_string(),
                            (kind.clone(), pallet.into(), item.into())
                        )
                        .is_none(),
                    "descriptor literal rebound"
                );
            }
        }
        let blob = resolver.tree.get(path).context("read source blob")?.clone();
        Ok(Self {
            path: path.into(),
            blob,
            source,
            suite,
            aliases,
            literals,
            functions,
        })
    }
    fn descriptor(&self, expr: &ast::Expr) -> Option<Hit> {
        let path = chain(expr)?;
        if path.len() == 1 {
            return self.literals.get(&path[0]).cloned();
        }
        if path.len() != 3 {
            return None;
        }
        let kind = match self.aliases.get(&path[0])?.as_str() {
            "storage" => "storage",
            "runtime_apis" => "runtime",
            "constants" => "constant",
            _ => return None,
        };
        Some((kind.into(), path[1].clone(), path[2].clone()))
    }
    fn scan(
        &self,
        body: &[ast::Stmt],
        hits: &mut BTreeSet<Hit>,
        seen: &mut BTreeSet<String>,
        pinned: &mut bool,
    ) -> Result<()> {
        let mut helpers = BTreeSet::new();
        super::super::effects::all_expressions(body, &mut |expr| {
            if let Some(hit) = self.descriptor(expr) {
                hits.insert(hit);
            }
            if let ast::Expr::Call(call) = expr
                && let Some(path) = chain(&call.func)
            {
                if path == ["view", "at"]
                    || path == ["view", "query_map"]
                    || path == ["view", "query_batch"]
                {
                    *pinned = true;
                }
                if path.len() == 1 && self.functions.contains_key(&path[0]) {
                    helpers.insert(path[0].clone());
                }
            }
        })?;
        for helper in helpers {
            if seen.insert(helper.clone()) {
                self.scan(&self.functions[&helper], hits, seen, pinned)?;
            }
        }
        Ok(())
    }
    fn registration(&self, start: usize, end: usize) -> Result<Vec<ast::Stmt>> {
        let mut matches = Vec::new();
        super::super::effects::all_expressions(&self.suite, &mut |expr| {
            if expr.range().start().to_usize() == start && expr.range().end().to_usize() == end {
                matches.push(ast::Stmt::Expr(ast::StmtExpr {
                    range: expr.range(),
                    value: Box::new(expr.clone()),
                }));
            }
        })?;
        // The walker can encounter a node twice as part of a containing expression.
        matches.truncate(1);
        ensure!(
            !matches.is_empty(),
            "{}: registration span no longer identifies an AST expression",
            self.path
        );
        Ok(matches)
    }
}

/// Produce static hit sets and source-derived rendering/pinning metadata.
pub(super) fn outputs(
    root: &Path,
    pins: &DocumentMut,
    _tables: &Value,
    params: &Value,
) -> Result<Vec<(&'static str, Vec<u8>)>> {
    let resolver = Resolver::new(root)?;
    let mut modules = BTreeMap::new();
    let mut reads = Vec::new();
    for row in params["reads"]
        .as_array()
        .context("read parameter records")?
    {
        let name = row["name"].as_str().context("read name")?;
        let declaration = &row["provenance"]["declaration"];
        let path = declaration["path"]
            .as_str()
            .context("read declaration path")?;
        if !modules.contains_key(path) {
            modules.insert(path.to_owned(), Module::load(&resolver, path)?);
        }
        let module = &modules[path];
        let mut hits = BTreeSet::new();
        let mut seen = BTreeSet::new();
        let mut pinned = false;
        let mut render = Value::Null;
        if row["provenance"]["fetch"]["path"].as_str() == Some(path) {
            let body = module
                .functions
                .get(name)
                .with_context(|| format!("missing read function {name}"))?;
            seen.insert(name.to_owned());
            module.scan(body, &mut hits, &mut seen, &mut pinned)?;
            for stmt in &module.suite {
                let decorators = match stmt {
                    ast::Stmt::AsyncFunctionDef(f) if f.name.as_str() == name => &f.decorator_list,
                    ast::Stmt::FunctionDef(f) if f.name.as_str() == name => &f.decorator_list,
                    _ => continue,
                };
                for decorator in decorators {
                    if let ast::Expr::Call(call) = decorator {
                        for keyword in &call.keywords {
                            if keyword
                                .arg
                                .as_ref()
                                .is_some_and(|arg| arg.as_str() == "render")
                                && let ast::Expr::Call(render_call) = &keyword.value
                            {
                                let kind =
                                    chain(&render_call.func).context("render constructor")?;
                                let labels = render_call
                                    .args
                                    .iter()
                                    .map(|arg| {
                                        text(arg).map(str::to_owned).context("render label literal")
                                    })
                                    .collect::<Result<Vec<_>>>()?;
                                render = json!({"kind":kind.last(),"labels":labels});
                            }
                        }
                    }
                }
            }
        } else {
            let start = usize::try_from(
                declaration["start_byte"]
                    .as_u64()
                    .context("read registration start")?,
            )?;
            let end = usize::try_from(
                declaration["end_byte"]
                    .as_u64()
                    .context("read registration end")?,
            )?;
            module.scan(
                &module.registration(start, end)?,
                &mut hits,
                &mut seen,
                &mut pinned,
            )?;
        }
        let hits = hits
            .into_iter()
            .map(|(kind, container, name)| json!({"kind":kind,"container":container,"name":name}))
            .collect::<Vec<_>>();
        reads.push(json!({"name":name,"hits":hits,"helpers":seen,"pinning":if pinned||hits.len()>1{"pinned"}else{"single"},"render":render,"provenance":{"path":module.path,"blob":module.blob,"sha256":format!("{:x}",sha2::Sha256::digest(module.source.as_bytes()))}}));
    }
    use sha2::Digest;
    ensure!(reads.len() == 102, "expected all102 read hit records");
    let document = json!({"format":1,"parity":{"tag":pins["parity"]["tag"].as_str(),"commit":pins["parity"]["commit"].as_str()},"reads":reads});
    Ok(vec![(
        "vendor/upstream/extracted/read_hits.json",
        pretty(&document)?,
    )])
}

#[cfg(test)]
mod tests {
    use super::*;
    fn document() -> Result<Value> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .context("workspace root")?;
        let pins = std::fs::read_to_string(root.join("upstream.toml"))?.parse()?;
        let params = serde_json::from_slice(&std::fs::read(
            root.join("vendor/upstream/extracted/read_params.json"),
        )?)?;
        let generated = outputs(root, &pins, &Value::Null, &params)?;
        serde_json::from_slice(&generated[0].1).map_err(Into::into)
    }
    fn read<'a>(document: &'a Value, name: &str) -> Result<&'a Value> {
        document["reads"]
            .as_array()
            .context("reads")?
            .iter()
            .find(|row| row["name"].as_str() == Some(name))
            .context("read absent")
    }
    fn hit(value: &Value) -> Result<Hit> {
        let field = |key: &str| {
            value[key]
                .as_str()
                .map(str::to_owned)
                .with_context(|| format!("hit {key}"))
        };
        Ok((field("kind")?, field("container")?, field("name")?))
    }
    #[test]
    fn transitive_helpers_and_module_literals_cover_catalog_omissions() -> Result<()> {
        let document = document()?;
        let lock = read(&document, "coldkey_lock")?;
        let hits = lock["hits"]
            .as_array()
            .context("hits")?
            .iter()
            .map(hit)
            .collect::<Result<BTreeSet<_>>>()?;
        ensure!(
            hits.contains(&(
                "runtime".into(),
                "StakeInfoRuntimeApi".into(),
                "get_coldkey_lock".into()
            )),
            "helper API missing"
        );
        ensure!(
            hits.contains(&(
                "storage".into(),
                "SubtensorModule".into(),
                "DecayingLock".into()
            )),
            "helper storage missing"
        );
        ensure!(
            lock["helpers"]
                .as_array()
                .context("helpers")?
                .contains(&json!("_lock_record")),
            "helper graph missing"
        );
        let collateral = read(&document, "miner_collateral")?;
        ensure!(
            collateral["hits"].as_array().context("hits")?.contains(
                &json!({"kind":"storage","container":"SubtensorModule","name":"MinerCollateral"})
            ),
            "Item literal missing"
        );
        let basket = read(&document, "basket_position")?;
        ensure!(basket["hits"].as_array().context("hits")?.contains(&json!({"kind":"runtime","container":"BetaBasketRuntimeApi","name":"get_basket_position"})),"Method literal missing");
        Ok(())
    }
    #[test]
    fn scalar_registrations_and_rendering_are_source_derived() -> Result<()> {
        let document = document()?;
        let scalar = read(&document, "max_weight_limit")?;
        ensure!(
            scalar["hits"]
                == json!([{"kind":"storage","container":"SubtensorModule","name":"MaxWeightsLimit"}]),
            "scalar descriptor changed"
        );
        let weights = read(&document, "weights")?;
        ensure!(
            weights["render"] == json!({"kind":"Matrix","labels":["validator","miner","weight"]}),
            "matrix render changed"
        );
        ensure!(weights["pinning"] == "pinned", "map scans must pin");
        ensure!(
            read(&document, "timestamp")?["hits"] == json!([]),
            "out-of-module touches must stay reviewed extras"
        );
        ensure!(
            document["reads"].as_array().context("reads")?.len() == 102,
            "read coverage changed"
        );
        Ok(())
    }
}
