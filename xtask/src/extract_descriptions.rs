//! Bounded static assembly of Python's per-pallet chain-error descriptions.

use anyhow::{Context, Result, bail, ensure};
use rustpython_parser::{
    Mode, Parse, Tok,
    ast::{self, Ranged},
    lexer,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use toml_edit::DocumentMut;

use super::{Resolver, document, line};

const DIRECTORY: &str = "sdk/python/bittensor/error_descriptions/";
const AGGREGATOR: &str = "sdk/python/bittensor/error_descriptions/__init__.py";

fn tokens(source: &str) -> Result<Vec<Tok>> {
    lexer::lex(source, Mode::Module)
        .map(|token| {
            token
                .map(|(token, _)| token)
                .map_err(|error| anyhow::anyhow!("{error:?}"))
        })
        .collect()
}

fn exact(source: &str, statement: &ast::Stmt, expected: &str) -> Result<()> {
    let range = statement.range();
    let text = &source[range.start().to_usize()..range.end().to_usize()];
    ensure!(
        tokens(text)? == tokens(expected)?,
        "{AGGREGATOR}:{}: description assembly changed; requires a reviewed static port",
        line(source, range.start().to_usize())
    );
    Ok(())
}

fn imports(source: &str) -> Result<Vec<(String, String)>> {
    let suite =
        ast::Suite::parse(source, AGGREGATOR).map_err(|error| anyhow::anyhow!("{error}"))?;
    let mut statements = suite.as_slice();
    if matches!(statements.first(),Some(ast::Stmt::Expr(expr)) if matches!(&*expr.value, ast::Expr::Constant(value) if matches!(value.value,ast::Constant::Str(_))))
    {
        statements = &statements[1..];
    }
    ensure!(
        statements.len() >= 5 && statements.len() <= 68,
        "description aggregator statement count exceeds checked grammar"
    );
    exact(source, &statements[0], "from __future__ import annotations")?;
    let mut result = Vec::new();
    let mut aliases = BTreeSet::new();
    let mut paths = BTreeSet::new();
    for statement in &statements[1..statements.len() - 3] {
        let ast::Stmt::ImportFrom(import) = statement else {
            bail!("description aggregator may only import literal leaf tables")
        };
        ensure!(
            import.level.is_some_and(|level| level.to_usize() == 1) && import.names.len() == 1,
            "description import must be one relative leaf table"
        );
        let module = import
            .module
            .as_ref()
            .context("description leaf module missing")?
            .as_str();
        ensure!(
            !module.is_empty()
                && module
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte == b'_'),
            "unsafe description leaf module"
        );
        let imported = &import.names[0];
        let alias = imported
            .asname
            .as_ref()
            .context("description import needs an explicit alias")?
            .to_string();
        ensure!(
            imported.name.as_str() == "DESCRIPTIONS"
                && alias.starts_with('_')
                && aliases.insert(alias.clone()),
            "invalid or duplicate description import alias"
        );
        let path = format!("{DIRECTORY}{module}.py");
        ensure!(
            paths.insert(path.clone()),
            "duplicate description leaf import"
        );
        result.push((path, alias));
    }
    let tail = &statements[statements.len() - 3..];
    exact(source, &tail[0], "DESCRIPTIONS: dict[str, str] = {}")?;
    let ast::Stmt::For(loop_) = &tail[1] else {
        bail!("description aggregator loop missing")
    };
    let ast::Expr::Tuple(parts) = &*loop_.iter else {
        bail!("description aggregation must iterate a literal tuple")
    };
    let mut ordered = Vec::new();
    let mut seen = BTreeSet::new();
    for part in &parts.elts {
        let ast::Expr::Name(name) = part else {
            bail!("description part must be an imported name")
        };
        let name = name.id.to_string();
        ensure!(
            aliases.contains(&name) && seen.insert(name.clone()),
            "unknown or repeated description part"
        );
        ordered.push(name);
    }
    ensure!(seen == aliases, "description loop omits an imported leaf");
    let expected = format!(
        "for _part in ({},):\n    overlap = DESCRIPTIONS.keys() & _part.keys()\n    if overlap:\n        raise RuntimeError(f\"duplicate error descriptions: {{sorted(overlap)}}\")\n    DESCRIPTIONS.update(_part)",
        ordered.join(",")
    );
    exact(source, &tail[1], &expected)?;
    exact(source, &tail[2], "__all__ = [\"DESCRIPTIONS\"]")?;
    let by_alias = result
        .into_iter()
        .map(|(path, alias)| (alias, path))
        .collect::<BTreeMap<_, _>>();
    Ok(ordered
        .into_iter()
        .map(|alias| (by_alias[&alias].clone(), alias))
        .collect())
}

fn compare(descriptions: &Value, catalog: &Value) -> Result<()> {
    let actual = descriptions["descriptions"]
        .as_object()
        .context("assembled descriptions missing")?;
    let expected = catalog["chain_errors"]
        .as_object()
        .context("catalog chain descriptions missing")?;
    ensure!(
        actual.keys().collect::<BTreeSet<_>>() == expected.keys().collect(),
        "assembled Python description name set differs from catalog"
    );
    for (name, row) in actual {
        ensure!(
            row["description"].as_str() == expected[name]["description"].as_str()
                && row["description"].is_string(),
            "Python description differs from catalog: {name}"
        );
    }
    Ok(())
}

pub(super) fn checked(
    resolver: &mut Resolver<'_>,
    pins: &DocumentMut,
    catalog: &Value,
) -> Result<Value> {
    let source = resolver.source(AGGREGATOR)?;
    let leaves = imports(&source)?;
    let mut descriptions = serde_json::Map::new();
    let mut sources = Vec::new();
    for (path, alias) in leaves {
        let leaf_source = resolver.source(&path)?;
        let suite =
            ast::Suite::parse(&leaf_source, &path).map_err(|error| anyhow::anyhow!("{error}"))?;
        let body = if matches!(suite.first(),Some(ast::Stmt::Expr(expr)) if matches!(&*expr.value,ast::Expr::Constant(value) if matches!(value.value,ast::Constant::Str(_))))
        {
            &suite[1..]
        } else {
            suite.as_slice()
        };
        ensure!(
            body.len() == 2,
            "{path}: description leaf must remain one annotation and literal table"
        );
        exact(&leaf_source, &body[0], "from __future__ import annotations")?;
        let ast::Stmt::AnnAssign(assign) = &body[1] else {
            bail!("{path}: description leaf annotation missing")
        };
        ensure!(
            matches!(&*assign.target,ast::Expr::Name(name) if name.id.as_str()=="DESCRIPTIONS"),
            "{path}: description assignment target changed"
        );
        let Some(ast::Expr::Dict(dictionary)) = assign.value.as_deref() else {
            bail!("{path}: descriptions must remain a literal dict")
        };
        let mut spans = BTreeMap::new();
        for (key, value) in dictionary.keys.iter().zip(&dictionary.values) {
            let Some(ast::Expr::Constant(key)) = key else {
                bail!("{path}: description key must be literal")
            };
            let ast::Constant::Str(name) = &key.value else {
                bail!("{path}: description key must be a string")
            };
            ensure!(
                matches!(value,ast::Expr::Constant(value) if matches!(value.value,ast::Constant::Str(_))),
                "{path}: description text must remain a literal string"
            );
            ensure!(
                spans
                    .insert(
                        name.clone(),
                        (
                            line(&leaf_source, key.range.start().to_usize()),
                            line(&leaf_source, value.range().end().to_usize())
                        )
                    )
                    .is_none(),
                "{path}: duplicate description key {name}"
            );
        }
        let table = document(resolver, pins, &path, &["DESCRIPTIONS"])?;
        let pairs = table["values"]["DESCRIPTIONS"]["value"]["dict"]
            .as_array()
            .context("description leaf did not resolve to dictionary")?;
        for pair in pairs {
            let name = pair[0]
                .as_str()
                .context("description name must be string")?;
            let description = pair[1]
                .as_str()
                .context("description text must be string")?;
            let (line, end_line) = spans.get(name).context("description entry span missing")?;
            ensure!(descriptions.insert(name.into(),json!({"description":description,"provenance":table["provenance"],"line":line,"end_line":end_line})).is_none(), "duplicate assembled description: {name}");
        }
        sources.push(json!({"alias":alias,"provenance":table["provenance"],"entries":pairs.len()}));
    }
    let output = json!({"format":1,"aggregator":{"path":AGGREGATOR,"blob":resolver.tree.get(AGGREGATOR),"tag":pins["parity"]["tag"].as_str(),"commit":pins["parity"]["commit"].as_str()},"sources":sources,"descriptions":descriptions,"catalog_equality":"checked"});
    compare(&output, catalog)?;
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::Path};

    #[test]
    fn description_assembly_rejects_changed_loop_and_catalog_text() -> Result<()> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .context("workspace root missing")?;
        let mut resolver = Resolver::new(root)?;
        let pins = fs::read_to_string(root.join("upstream.toml"))?.parse::<DocumentMut>()?;
        let catalog: Value =
            serde_json::from_slice(&fs::read(root.join("vendor/upstream/catalog/errors.json"))?)?;
        let descriptions = checked(&mut resolver, &pins, &catalog)?;
        assert_eq!(
            descriptions["descriptions"]
                .as_object()
                .context("description map missing")?
                .len(),
            392
        );
        let source = resolver.source(AGGREGATOR)?;
        for changed in [
            source.replace("DESCRIPTIONS.update(_part)", "DESCRIPTIONS.update({})"),
            source.replace("if overlap:", "if not overlap:"),
            source.replace("    _UTILITY,", "    _SYSTEM,"),
            source.replace(
                "__all__ = [\"DESCRIPTIONS\"]",
                "DESCRIPTIONS.clear()\n__all__ = [\"DESCRIPTIONS\"]",
            ),
        ] {
            assert!(imports(&changed).is_err());
        }
        let leaf = "sdk/python/bittensor/error_descriptions/balances.py";
        let source = resolver.source(leaf)?;
        for changed in [
            source.replacen("\"DeadAccount\":", "\"DeltaZero\":", 1),
            source.replacen(
                "The beneficiary account does not exist",
                "Changed beneficiary meaning",
                1,
            ),
        ] {
            let mut resolver = Resolver::new(root)?;
            resolver.sources.insert(leaf.into(), changed);
            assert!(checked(&mut resolver, &pins, &catalog).is_err());
        }
        let mut changed = catalog.clone();
        changed["chain_errors"]["SlippageTooHigh"]["description"] = json!("changed");
        assert!(compare(&descriptions, &changed).is_err());
        let mut changed = descriptions;
        changed["descriptions"]
            .as_object_mut()
            .context("description map missing")?
            .remove("SlippageTooHigh");
        assert!(compare(&changed, &catalog).is_err());
        Ok(())
    }
}
