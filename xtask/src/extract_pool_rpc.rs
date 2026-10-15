//! Authorship pool codes from the Cargo-locked Polkadot SDK Git objects.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail, ensure};
use quote::ToTokens;
use serde_json::{Value, json};
use syn::{Expr, Item, Pat, Stmt};
use toml_edit::DocumentMut;

use super::{pretty, rust_string};

const REPOSITORY: &str = "https://github.com/RaoFoundation/polkadot-sdk.git";
const BASE_PATH: &str = "substrate/client/rpc-api/src/error.rs";
const AUTHOR_PATH: &str = "substrate/client/rpc-api/src/author/error.rs";
type GeneratedOutputs = Vec<(&'static str, Vec<u8>)>;

fn locked_commit(root: &Path) -> Result<String> {
    let lock = fs::read_to_string(root.join("Cargo.lock"))?.parse::<DocumentMut>()?;
    let prefix = format!("git+{REPOSITORY}?rev=");
    let mut commits = BTreeSet::new();
    for package in lock["package"]
        .as_array_of_tables()
        .context("lock packages missing")?
    {
        if let Some(source) = package.get("source").and_then(toml_edit::Item::as_str)
            && let Some(pin) = source.strip_prefix(&prefix)
        {
            let (requested, actual) = pin
                .split_once('#')
                .context("SDK lock source missing commit")?;
            ensure!(
                requested == actual
                    && actual.len() == 40
                    && actual
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
                "SDK must use one exact full revision"
            );
            commits.insert(actual.to_owned());
        }
    }
    ensure!(
        commits.len() == 1,
        "lock must contain one authoritative Polkadot SDK revision"
    );
    commits.into_iter().next().context("SDK revision absent")
}

fn git_database(commit: &str) -> Result<PathBuf> {
    let cargo_home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cargo")))
        .context("Cargo home unavailable")?;
    let mut databases = fs::read_dir(cargo_home.join("git/db"))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    databases.sort();
    for database in databases {
        if !database
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("polkadot-sdk-"))
        {
            continue;
        }
        let status = Command::new("git")
            .arg("-C")
            .arg(&database)
            .args(["cat-file", "-e", &format!("{commit}^{{commit}}")])
            .output()?;
        if status.status.success() {
            return Ok(database);
        }
    }
    bail!(
        "Cargo's SDK Git database lacks the locked revision; fetch locked dependencies before extraction"
    )
}

fn git_source(database: &Path, commit: &str, path: &str) -> Result<(String, Value)> {
    let object = format!("{commit}:{path}");
    let output = Command::new("git")
        .arg("-C")
        .arg(database)
        .args(["show", &object])
        .output()?;
    ensure!(
        output.status.success() && output.stdout.len() <= super::MAX_SOURCE_BYTES,
        "locked SDK source missing or exceeds byte bound: {path}"
    );
    let blob = super::super::git_blob(&output.stdout);
    let identity = Command::new("git")
        .arg("-C")
        .arg(database)
        .args(["rev-parse", &object])
        .output()?;
    ensure!(
        identity.status.success() && String::from_utf8_lossy(&identity.stdout).trim() == blob,
        "SDK source bytes differ from locked Git blob"
    );
    Ok((
        String::from_utf8(output.stdout)?,
        json!({"repository":REPOSITORY,"commit":commit,"path":path,"blob":blob}),
    ))
}

fn tokens(value: &impl ToTokens) -> String {
    value.to_token_stream().to_string()
}

fn number(
    expr: &Expr,
    constants: &BTreeMap<String, syn::ItemConst>,
    values: &mut BTreeMap<String, i64>,
    visiting: &mut BTreeSet<String>,
) -> Result<i64> {
    let result = match expr {
        Expr::Lit(literal) => match &literal.lit {
            syn::Lit::Int(integer) if integer.suffix().is_empty() || integer.suffix() == "i32" => {
                integer.base10_parse()?
            }
            _ => bail!("pool RPC value must be an i32 integer"),
        },
        Expr::Path(path) if path.qself.is_none() => {
            let name = tokens(&path.path).replace(' ', "");
            if let Some(value) = values.get(&name) {
                return Ok(*value);
            }
            ensure!(
                visiting.insert(name.clone()),
                "cyclic pool RPC constant {name}"
            );
            let constant = constants
                .get(&name)
                .with_context(|| format!("unresolved pool RPC constant {name}"))?;
            let result = number(&constant.expr, constants, values, visiting)?;
            visiting.remove(&name);
            values.insert(name, result);
            result
        }
        Expr::Binary(binary) if matches!(binary.op, syn::BinOp::Add(_)) => {
            number(&binary.left, constants, values, visiting)?
                .checked_add(number(&binary.right, constants, values, visiting)?)
                .context("pool RPC arithmetic overflow")?
        }
        _ => bail!("pool RPC arithmetic changed outside checked literals, names and addition"),
    };
    ensure!(i32::try_from(result).is_ok(), "pool RPC value exceeds i32");
    Ok(result)
}

fn pool_variant(pattern: &Pat) -> Option<String> {
    let Pat::TupleStruct(outer) = pattern else {
        return None;
    };
    if tokens(&outer.path).replace(' ', "") != "Error::Pool" || outer.elems.len() != 1 {
        return None;
    }
    let path = match outer.elems.first()? {
        Pat::TupleStruct(value) => &value.path,
        Pat::Struct(value) => &value.path,
        Pat::Path(value) => &value.path,
        _ => return None,
    };
    if path.segments.len() != 2 || path.segments.first()?.ident != "PoolError" {
        return None;
    }
    Some(path.segments.last()?.ident.to_string())
}

fn returned_code(expr: &Expr, constants: &BTreeMap<String, syn::ItemConst>) -> Result<String> {
    let expr = if let Expr::Block(block) = expr {
        let (last, preceding) = block
            .block
            .stmts
            .split_last()
            .context("empty pool RPC arm")?;
        for statement in preceding {
            let Stmt::Local(local) = statement else {
                bail!("pool RPC arm gained preceding effects");
            };
            let pattern = match &local.pat {
                Pat::Type(pattern) => &*pattern.pat,
                pattern => pattern,
            };
            let Pat::Ident(name) = pattern else {
                bail!("pool RPC arm local binding changed");
            };
            ensure!(
                !constants.contains_key(&name.ident.to_string()) && name.ident != "ErrorObject",
                "pool RPC arm shadows authoritative code or constructor"
            );
        }
        let Stmt::Expr(expr, None) = last else {
            bail!("pool RPC arm lacks its direct return");
        };
        expr
    } else {
        expr
    };
    let Expr::Call(call) = expr else {
        bail!("pool RPC conversion must directly call ErrorObject::owned");
    };
    ensure!(
        tokens(&call.func).replace(' ', "") == "ErrorObject::owned" && call.args.len() == 3,
        "pool RPC constructor changed"
    );
    let Some(Expr::Path(path)) = call.args.first() else {
        bail!("pool RPC code must be an authoritative constant");
    };
    let name = tokens(&path.path).replace(' ', "");
    ensure!(
        name.starts_with("POOL_") && constants.contains_key(&name),
        "pool RPC arm uses an unknown code"
    );
    Ok(name)
}

fn parse(base: &str, author: &str) -> Result<Value> {
    let base = syn::parse_file(base)?;
    let modules = base
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Mod(module) if module.ident == "base" => Some(module),
            _ => None,
        })
        .collect::<Vec<_>>();
    ensure!(modules.len() == 1, "RPC base module missing or duplicated");
    let items = &modules[0]
        .content
        .as_ref()
        .context("RPC base module must be inline")?
        .1;
    let author_base = items
        .iter()
        .filter_map(|item| match item {
            Item::Const(value) if value.ident == "AUTHOR" => Some(value),
            _ => None,
        })
        .collect::<Vec<_>>();
    ensure!(
        author_base.len() == 1 && tokens(&author_base[0].ty) == "i32",
        "AUTHOR must remain one i32 constant"
    );
    let author_base = number(
        &author_base[0].expr,
        &BTreeMap::new(),
        &mut BTreeMap::new(),
        &mut BTreeSet::new(),
    )?;
    let author = syn::parse_file(author)?;
    let mut constants = BTreeMap::new();
    for item in &author.items {
        if let Item::Const(constant) = item {
            ensure!(
                tokens(&constant.ty) == "i32",
                "author error code must be i32"
            );
            ensure!(
                constants
                    .insert(constant.ident.to_string(), constant.clone())
                    .is_none(),
                "duplicate author constant"
            );
        }
    }
    ensure!(
        constants.len() <= 128,
        "author constants exceed extraction bound"
    );
    let mut values = BTreeMap::from([("crate::error::base::AUTHOR".to_owned(), author_base)]);
    for constant in constants.values() {
        number(
            &constant.expr,
            &constants,
            &mut values,
            &mut BTreeSet::new(),
        )?;
    }
    let conversions =
        author
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Impl(implementation)
                    if implementation.trait_.as_ref().is_some_and(|(_, path, _)| {
                        tokens(path).replace(' ', "") == "From<Error>"
                    }) && tokens(&implementation.self_ty) == "ErrorObjectOwned" =>
                {
                    Some(implementation)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
    ensure!(
        conversions.len() == 1,
        "author error conversion missing or duplicated"
    );
    let functions = conversions[0]
        .items
        .iter()
        .filter_map(|item| match item {
            syn::ImplItem::Fn(function) if function.sig.ident == "from" => Some(function),
            _ => None,
        })
        .collect::<Vec<_>>();
    ensure!(
        functions.len() == 1,
        "author error conversion function missing or duplicated"
    );
    let body = &functions[0].block.stmts;
    ensure!(
        body.len() == 2
            && matches!(&body[0],Stmt::Item(Item::Use(usage)) if tokens(&usage.tree).replace(' ',"")=="sc_transaction_pool_api::error::ErrorasPoolError"),
        "PoolError alias or conversion body changed"
    );
    let Stmt::Expr(Expr::Match(branches), None) = &body[1] else {
        bail!("author error conversion must return its match");
    };
    ensure!(
        tokens(&branches.expr) == "e",
        "author error match input changed"
    );
    let mut rows = BTreeMap::new();
    for arm in &branches.arms {
        if let Some(variant) = pool_variant(&arm.pat) {
            ensure!(arm.guard.is_none(), "pool RPC match gained a guard");
            let constant = returned_code(&arm.body, &constants)?;
            let code = number(
                &syn::parse_str::<Expr>(&constant)?,
                &constants,
                &mut values,
                &mut BTreeSet::new(),
            )?;
            let row = json!({"code":code,"constant":constant,"rust_variant":variant});
            if let Some(previous) = rows.insert(variant, row.clone()) {
                ensure!(previous == row, "pool variant maps to conflicting codes");
            }
        }
    }
    ensure!(!rows.is_empty(), "pool RPC arm inventory empty");
    let mut used = BTreeSet::new();
    for row in rows.values() {
        ensure!(
            used.insert(row["code"].as_i64().context("pool code invalid")?),
            "distinct pool variants share a code"
        );
    }
    let pool_constants = constants
        .keys()
        .filter(|name| name.starts_with("POOL_"))
        .cloned()
        .collect::<BTreeSet<_>>();
    ensure!(
        pool_constants
            == rows
                .values()
                .filter_map(|row| row["constant"].as_str().map(str::to_owned))
                .collect(),
        "pool code constant has no conversion arm"
    );
    Ok(json!({"base":author_base,"variants":rows}))
}

pub(super) fn outputs(root: &Path) -> Result<(String, GeneratedOutputs)> {
    let commit = locked_commit(root)?;
    let database = git_database(&commit)?;
    let (base, base_provenance) = git_source(&database, &commit, BASE_PATH)?;
    let (author, author_provenance) = git_source(&database, &commit, AUTHOR_PATH)?;
    let rules = parse(&base, &author)?;
    let document = json!({"format":1,"provenance":{"base":base_provenance,"author":author_provenance},"rules":rules,"projection":{"RejectedFutureTransaction":"FutureTx"}});
    let mut rust = format!(
        "\n// Authorship pool codes from {REPOSITORY} at {commit}.\n// Parsed from {AUTHOR_PATH} and {BASE_PATH}; source license GPL-3.0-or-later WITH Classpath-exception-2.0.\n/// All exact node pool error codes and their Rust variant names.\npub const POOL_RPC_ALL_CODES: &[(i64,&str)] = &[\n"
    );
    for (name, row) in rules["variants"].as_object().context("pool rows missing")? {
        rust.push_str(&format!("({},{}),\n", row["code"], rust_string(name)));
    }
    rust.push_str("];\n/// Pool rejection codes with stable client names; the runtime future variant projects to FutureTx.\npub const POOL_RPC_CODES: &[(i64,&str)] = &[\n");
    for (source, name) in [
        ("TemporarilyBanned", "TemporarilyBanned"),
        ("AlreadyImported", "AlreadyImported"),
        ("TooLowPriority", "TooLowPriority"),
        ("RejectedFutureTransaction", "FutureTx"),
    ] {
        let code = rules["variants"][source]["code"]
            .as_i64()
            .with_context(|| format!("required pool rejection {source} disappeared"))?;
        rust.push_str(&format!("({code},{}),\n", rust_string(name)));
    }
    rust.push_str("];\n");
    Ok((
        rust,
        vec![(
            "vendor/upstream/extracted/pool_rpc_codes.json",
            pretty(&document)?,
        )],
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn locked_sdk_pool_codes_follow_constant_arithmetic_and_exact_conversion_arms() -> Result<()> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .context("workspace absent")?;
        let commit = locked_commit(root)?;
        let database = git_database(&commit)?;
        let (base, _) = git_source(&database, &commit, BASE_PATH)?;
        let (author, _) = git_source(&database, &commit, AUTHOR_PATH)?;
        let rows = parse(&base, &author)?;
        assert_eq!(rows["base"], json!(1000));
        for (name, code) in [
            ("TemporarilyBanned", 1012),
            ("AlreadyImported", 1013),
            ("TooLowPriority", 1014),
            ("RejectedFutureTransaction", 1021),
        ] {
            assert_eq!(rows["variants"][name]["code"], json!(code));
        }
        assert_eq!(
            rows["variants"]
                .as_object()
                .context("variants absent")?
                .len(),
            11
        );
        assert!(
            parse(
                &base,
                &author.replace("POOL_INVALID_TX + 2", "POOL_INVALID_TX * 2")
            )
            .is_err()
        );
        assert!(
            parse(
                &base,
                &author.replace("POOL_TEMPORARILY_BANNED,", "POOL_ALREADY_IMPORTED,")
            )
            .is_err()
        );
        assert!(parse(&base, &author.replace("POOL_FUTURE_TX,", "BASE_ERROR,")).is_err());
        Ok(())
    }
}
