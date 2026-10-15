//! Parse Python's generated wire layer and compare every metadata name/argument.

use anyhow::{Context, Result, bail, ensure};
use rtao_core::ir::MetadataIr;
use rustpython_parser::{Parse, ast};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

fn suite(root: &Path, name: &str) -> Result<ast::Suite> {
    let path = root.join(format!(
        "vendor/upstream/src/sdk/python/bittensor/_generated/{name}.py"
    ));
    let source = fs::read_to_string(&path)?;
    ensure!(
        source.len() <= 8 * 1024 * 1024,
        "generated Python source exceeds bound"
    );
    ast::Suite::parse(&source, &path.to_string_lossy()).map_err(Into::into)
}
fn string(expression: &ast::Expr) -> Result<String> {
    match expression {
        ast::Expr::Constant(constant) => match &constant.value {
            ast::Constant::Str(value) => Ok(value.clone()),
            _ => bail!("generated descriptor value not string"),
        },
        _ => bail!("generated descriptor expression not literal"),
    }
}
fn number(expression: &ast::Expr) -> Result<u8> {
    match expression {
        ast::Expr::Constant(constant) => match &constant.value {
            ast::Constant::Int(value) => value.to_string().parse().context("index outside u8"),
            _ => bail!("index not integer"),
        },
        _ => bail!("index not literal"),
    }
}
fn descriptors(
    root: &Path,
    name: &str,
    constructor: &str,
) -> Result<BTreeMap<(String, String), Vec<String>>> {
    let mut found = BTreeMap::new();
    for item in suite(root, name)? {
        let ast::Stmt::ClassDef(class) = item else {
            continue;
        };
        for item in class.body {
            let ast::Stmt::Assign(assign) = item else {
                continue;
            };
            let ast::Expr::Call(call) = assign.value.as_ref() else {
                continue;
            };
            let ast::Expr::Name(called) = call.func.as_ref() else {
                continue;
            };
            if called.id.as_str() != constructor {
                continue;
            }
            ensure!(
                call.keywords.is_empty() && call.args.len() >= 2,
                "unsupported generated descriptor call"
            );
            let values = call.args.iter().map(string).collect::<Result<Vec<_>>>()?;
            let key = (values[0].clone(), values[1].clone());
            ensure!(
                key.0 == class.name.as_str(),
                "generated descriptor class mismatch"
            );
            ensure!(
                found
                    .insert(key, values.into_iter().skip(2).collect())
                    .is_none(),
                "duplicate Python descriptor"
            );
        }
    }
    Ok(found)
}
pub(super) fn check(root: &Path, ir: &MetadataIr, covered: &[String]) -> Result<()> {
    // COVERED_PALLETS must remain exactly upstream's declared literal scope.
    let source = fs::read_to_string(root.join("vendor/upstream/src/sdk/python/codegen/check.py"))?;
    let parsed = ast::Suite::parse(&source, "codegen/check.py")?;
    let expression=parsed.iter().find_map(|item|match item {ast::Stmt::Assign(assign) if assign.targets.iter().any(|target|matches!(target,ast::Expr::Name(name) if name.id.as_str()=="COVERED_PALLETS"))=>Some(assign.value.as_ref()),_=>None}).context("covered pallet source missing")?;
    let ast::Expr::Tuple(tuple) = expression else {
        bail!("covered pallet source no longer literal tuple")
    };
    ensure!(
        tuple.elts.iter().map(string).collect::<Result<Vec<_>>>()? == covered,
        "configured covered pallets differ from upstream"
    );
    let mut calls = BTreeMap::new();
    let pallet_names: BTreeSet<_> = ir.pallets.iter().map(|p| p.name.as_str()).collect();
    for item in suite(root, "calls")? {
        let ast::Stmt::ClassDef(class) = item else {
            continue;
        };
        if !pallet_names.contains(class.name.as_str()) {
            continue;
        }
        for item in class.body {
            let ast::Stmt::FunctionDef(function) = item else {
                continue;
            };
            ensure!(
                function.args.vararg.is_none()
                    && function.args.kwarg.is_none()
                    && function.args.kwonlyargs.is_empty(),
                "generated call has dynamic arguments"
            );
            let names = function
                .args
                .posonlyargs
                .iter()
                .chain(&function.args.args)
                .map(|argument| {
                    argument
                        .def
                        .arg
                        .to_string()
                        .trim_end_matches('_')
                        .to_owned()
                })
                .collect::<Vec<_>>();
            let key = (
                class.name.to_string(),
                function.name.to_string().trim_end_matches('_').to_owned(),
            );
            ensure!(
                calls.insert(key, names).is_none(),
                "duplicate generated Python call"
            );
        }
    }
    let expected_calls = ir
        .pallets
        .iter()
        .flat_map(|p| {
            p.calls.iter().map(move |call| {
                (
                    (p.name.clone(), call.name.clone()),
                    call.args
                        .iter()
                        .map(|field| field.name.clone())
                        .collect::<Vec<_>>(),
                )
            })
        })
        .collect::<BTreeMap<_, _>>();
    ensure!(
        calls == expected_calls,
        "Python generated call names/argument order differ from pinned metadata"
    );
    let storage = descriptors(root, "storage", "Item")?;
    let expected_storage = ir
        .pallets
        .iter()
        .flat_map(|p| {
            p.storage.iter().map(move |entry| {
                (
                    (p.name.clone(), entry.name.clone()),
                    vec![entry.value_type_ident.clone()],
                )
            })
        })
        .collect::<BTreeMap<_, _>>();
    ensure!(
        storage == expected_storage,
        "Python storage names/value identities differ from pinned metadata"
    );
    let constants = descriptors(root, "constants", "Item")?;
    let expected_constants = ir
        .pallets
        .iter()
        .flat_map(|p| {
            p.constants
                .iter()
                .map(move |name| ((p.name.clone(), name.clone()), Vec::new()))
        })
        .collect::<BTreeMap<_, _>>();
    ensure!(
        constants == expected_constants,
        "Python constant names differ from pinned metadata"
    );
    let apis = descriptors(root, "runtime_apis", "Method")?;
    let expected_apis = ir
        .runtime_apis
        .iter()
        .flat_map(|api| {
            api.methods
                .iter()
                .map(move |name| ((api.name.clone(), name.clone()), Vec::new()))
        })
        .collect::<BTreeMap<_, _>>();
    ensure!(
        apis == expected_apis,
        "Python runtime API names differ from pinned metadata"
    );
    let parsed = suite(root, "errors")?;
    let dictionary=parsed.iter().find_map(|item|match item {ast::Stmt::AnnAssign(assign) if matches!(assign.target.as_ref(),ast::Expr::Name(name) if name.id.as_str()=="ERRORS")=>assign.value.as_deref(),_=>None}).context("Python ERRORS missing")?;
    let ast::Expr::Dict(dictionary) = dictionary else {
        bail!("ERRORS no longer literal dictionary")
    };
    let mut errors = BTreeMap::new();
    for (key, value) in dictionary.keys.iter().zip(&dictionary.values) {
        let ast::Expr::Tuple(key) = key.as_ref().context("error key missing")? else {
            bail!("error key not tuple")
        };
        ensure!(key.elts.len() == 2, "error key length");
        let ast::Expr::Call(call) = value else {
            bail!("error row not constructor")
        };
        ensure!(call.args.len() == 3, "error row arguments");
        let index = (number(&key.elts[0])?, number(&key.elts[1])?);
        let row = (
            string(&call.args[0])?,
            string(&call.args[1])?,
            string(&call.args[2])?,
        );
        ensure!(errors.insert(index, row).is_none(), "duplicate error row");
    }
    let expected_errors = ir
        .pallets
        .iter()
        .flat_map(|p| {
            p.errors.iter().map(move |e| {
                (
                    (p.index, e.index),
                    (p.name.clone(), e.name.clone(), e.docs.clone()),
                )
            })
        })
        .collect::<BTreeMap<_, _>>();
    ensure!(
        errors == expected_errors,
        "Python error names/indices/docs differ from pinned metadata"
    );
    println!(
        "cross-checked Python: {} calls, {} storage entries, {} constants, {} API methods, {} errors",
        calls.len(),
        storage.len(),
        constants.len(),
        apis.len(),
        errors.len()
    );
    Ok(())
}
