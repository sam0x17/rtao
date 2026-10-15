//! Read namespace accessors from pinned categories and Python class literals.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use anyhow::{Context, Result, bail, ensure};
use rustpython_parser::{Parse, ast};
use serde_json::Value;
use sha2::{Digest, Sha256};
use toml_edit::{DocumentMut, Item};

const PYTHON: &str = "sdk/python/bittensor/namespaces.py";
const CATALOG: &str = "website/apps/bittensor-website/public/catalog/reads.json";
const OUTPUT: &str = "crates/rtao-reads/src/generated";

#[derive(Debug, PartialEq, Eq)]
struct Namespace {
    accessor: String,
    class: String,
    category: String,
}

fn name(expression: &ast::Expr) -> Option<&str> {
    if let ast::Expr::Name(expression) = expression {
        Some(expression.id.as_str())
    } else {
        None
    }
}

fn text(expression: &ast::Expr) -> Result<&str> {
    match expression {
        ast::Expr::Constant(expression) => match &expression.value {
            ast::Constant::Str(value) => Ok(value),
            _ => bail!("namespace category/key must be a literal string"),
        },
        _ => bail!("namespace category/key must be a literal string"),
    }
}

fn identifier(value: &str) -> Result<()> {
    ensure!(
        value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            && ![
                "self", "Self", "super", "crate", "mod", "type", "impl", "fn", "async", "await",
                "use", "trait"
            ]
            .contains(&value),
        "namespace identifier is not a portable Rust identifier: {value}"
    );
    Ok(())
}

fn discover(source: &str, catalog: &Value) -> Result<Vec<Namespace>> {
    ensure!(
        source.len() <= 1024 * 1024,
        "namespace source exceeds bounds"
    );
    let suite = ast::Suite::parse(source, PYTHON)?;
    let mut classes = BTreeMap::new();
    let mut table = None;
    for (index, statement) in suite.iter().enumerate() {
        match statement {
            ast::Stmt::Import(_) | ast::Stmt::ImportFrom(_) => {
                ensure!(
                    classes.is_empty(),
                    "namespace imports must precede class declarations"
                );
            }
            ast::Stmt::FunctionDef(function) => {
                ensure!(
                    function.decorator_list.is_empty()
                        && !classes.contains_key(function.name.as_str())
                        && function.name.as_str() != "NAMESPACES",
                    "decorated module function or namespace rebinding requires review"
                );
            }
            ast::Stmt::AsyncFunctionDef(function) => {
                ensure!(
                    function.decorator_list.is_empty()
                        && !classes.contains_key(function.name.as_str())
                        && function.name.as_str() != "NAMESPACES",
                    "decorated module function or namespace rebinding requires review"
                );
            }
            ast::Stmt::Expr(expression) => {
                text(&expression.value)?;
            }
            ast::Stmt::ClassDef(class) => {
                ensure!(
                    class.decorator_list.is_empty(),
                    "decorated namespace class requires review"
                );
                identifier(class.name.as_str())?;
                let mut category = None;
                for statement in &class.body {
                    let value = match statement {
                        ast::Stmt::Assign(value)
                            if value
                                .targets
                                .iter()
                                .any(|target| name(target) == Some("_category")) =>
                        {
                            ensure!(
                                value.targets.len() == 1,
                                "aliased namespace category requires review"
                            );
                            Some(value.value.as_ref())
                        }
                        ast::Stmt::AnnAssign(value) if name(&value.target) == Some("_category") => {
                            value.value.as_deref()
                        }
                        ast::Stmt::FunctionDef(function) => {
                            ensure!(
                                function.name.as_str() != "_category",
                                "namespace method rebinds its category"
                            );
                            None
                        }
                        ast::Stmt::AsyncFunctionDef(function) => {
                            ensure!(
                                function.name.as_str() != "_category",
                                "namespace method rebinds its category"
                            );
                            None
                        }
                        ast::Stmt::Expr(expression) => {
                            text(&expression.value)?;
                            None
                        }
                        ast::Stmt::Pass(_) => None,
                        _ => bail!("unsupported executable namespace class statement"),
                    };
                    if let Some(value) = value {
                        ensure!(category.is_none(), "namespace category is rebound");
                        category = Some(text(value)?.to_owned());
                    }
                }
                ensure!(
                    classes.insert(class.name.to_string(), category).is_none(),
                    "namespace class is rebound"
                );
            }
            ast::Stmt::AnnAssign(value) if name(&value.target) == Some("NAMESPACES") => {
                ensure!(
                    index + 1 == suite.len() && table.is_none(),
                    "namespace table must be the final declaration"
                );
                table = Some(
                    value
                        .value
                        .as_deref()
                        .context("namespace table has no value")?,
                );
            }
            _ => bail!("unsupported executable namespace module statement"),
        }
    }
    let ast::Expr::Dict(table) = table.context("namespace table missing")? else {
        bail!("namespace table must be a literal dictionary")
    };
    ensure!(
        table.keys.len() == table.values.len(),
        "namespace dictionary keys/values disagree"
    );
    let mut output = Vec::new();
    let mut accessors = BTreeSet::new();
    let mut categories = BTreeMap::new();
    for (key, value) in table.keys.iter().zip(&table.values) {
        let accessor = text(
            key.as_ref()
                .context("namespace dictionary expansion is unsupported")?,
        )?;
        identifier(accessor)?;
        let class = name(value).context("namespace table value must name a declared class")?;
        let category = classes
            .get(class)
            .and_then(Option::as_ref)
            .context("namespace class has no literal category")?;
        ensure!(
            accessors.insert(accessor.to_owned()),
            "duplicate namespace accessor"
        );
        ensure!(
            categories.insert(category.as_str(), accessor).is_none(),
            "category maps to multiple namespaces"
        );
        output.push(Namespace {
            accessor: accessor.to_owned(),
            class: class.to_owned(),
            category: category.clone(),
        });
    }
    let rows = catalog
        .as_array()
        .context("read catalog must be an array")?;
    let mut covered = BTreeSet::new();
    for row in rows {
        let category = row
            .get("category")
            .and_then(Value::as_str)
            .context("read category missing")?;
        let accessor = categories
            .get(category)
            .context("read category has no namespace")?;
        covered.insert(category);
        if let Some(python) = row
            .get("python")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            ensure!(
                python.starts_with(&format!("sub.{accessor}.")),
                "Python read prefix disagrees with catalog category: {python}"
            );
        }
    }
    ensure!(
        covered == categories.keys().copied().collect(),
        "namespace/category coverage differs"
    );
    output.sort_by(|left, right| left.accessor.cmp(&right.accessor));
    Ok(output)
}

fn render(namespaces: &[Namespace], commit: &str) -> String {
    let mut source = format!(
        "// Derived from RaoFoundation/subtensor {PYTHON} @ {commit} (Apache-2.0); modified by rtao.\n// @generated by cargo xtask regen. Do not edit.\n//! Read namespaces checked against catalog categories.\n\n"
    );
    for namespace in namespaces {
        let Namespace {
            accessor: _,
            class,
            category,
        } = namespace;
        source.push_str(&format!("/// Reads in the {category} category.\n#[doc(alias = \"bittensor.namespaces.{class}\")]\n#[derive(Clone, Copy)]\npub struct {class}<'a, V: ?Sized> {{ pub(crate) view: &'a V }}\n\nimpl<'a,V:?Sized> {class}<'a,V> {{\n/// Catalog category for this namespace.\npub const CATEGORY: &'static str = {category:?};\n/// Bind methods to a view without performing I/O.\npub const fn new(view:&'a V)->Self {{ Self{{view}} }}\n/// Borrow the view used by curated extension traits.\npub const fn view(&self)->&'a V {{ self.view }}\n}}\n\n"));
    }
    source.push_str("/// Catalog-derived namespace accessors on clients and snapshots.\npub trait Namespaces: rtao_client::View {\n");
    for Namespace {
        accessor,
        class,
        category,
    } in namespaces
    {
        source.push_str(&format!("/// Bind {category} reads to this view.\nfn {accessor}(&self)->{class}<'_,Self> {{ {class}::new(self) }}\n"));
    }
    source.push_str("}\nimpl<V:rtao_client::View+?Sized> Namespaces for V {}\n");
    source
}

fn input(root: &Path, pins: &DocumentMut, source: &str) -> Result<Vec<u8>> {
    let artifact = pins
        .get("artifacts")
        .and_then(|table| table.get(source))
        .context("namespace input artifact is unpinned")?;
    let destination = artifact
        .get("to")
        .and_then(Item::as_str)
        .context("namespace input destination missing")?;
    ensure!(
        destination.starts_with("vendor/upstream/")
            && !destination.split('/').any(|part| part == ".."),
        "namespace input path is outside the import tree"
    );
    let path = root.join(destination);
    ensure!(
        fs::symlink_metadata(&path)?.file_type().is_file(),
        "namespace input must be a regular file"
    );
    let bytes = fs::read(path)?;
    ensure!(
        bytes.len() <= 8 * 1024 * 1024,
        "namespace input exceeds bounds"
    );
    let hash = format!("{:x}", Sha256::digest(&bytes));
    ensure!(
        artifact.get("sha256").and_then(Item::as_str) == Some(hash.as_str()),
        "namespace input differs from pinned SHA256"
    );
    Ok(bytes)
}

pub(super) fn run(root: &Path, pins: &DocumentMut, check: bool) -> Result<()> {
    let source = String::from_utf8(input(root, pins, PYTHON)?)?;
    let catalog = serde_json::from_slice(&input(root, pins, CATALOG)?)?;
    let namespaces = discover(&source, &catalog)?;
    let commit = pins
        .get("parity")
        .and_then(|item| item.get("commit"))
        .and_then(Item::as_str)
        .context("namespace parity commit missing")?;
    let output = super::format_rust(&render(&namespaces, commit))?;
    let directory = root.join(OUTPUT);
    if !check {
        fs::create_dir_all(&directory)?;
    }
    ensure!(
        fs::symlink_metadata(&directory)?.file_type().is_dir(),
        "namespace output directory must be regular"
    );
    let path = directory.join("namespaces.rs");
    if let Ok(metadata) = fs::symlink_metadata(&path) {
        ensure!(
            metadata.file_type().is_file(),
            "namespace output must be regular"
        );
    }
    if check {
        ensure!(
            fs::read_to_string(path)? == output,
            "generated namespace accessors are stale; run cargo xtask regen"
        );
    } else {
        crate::pin::atomic_write(&path, output.as_bytes())?;
    }
    println!(
        "Verified {} catalog-derived namespace accessors",
        namespaces.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const SOURCE: &str =
        "class Subnets:\n    _category = 'Subnets'\nNAMESPACES: dict = {'subnets': Subnets}\n";
    fn catalog() -> Value {
        json!([{"category":"Subnets","python":"sub.subnets.token_symbols(...)"},{"category":"Subnets","python":""}])
    }

    #[test]
    fn categories_drive_accessors_without_shadowing_curated_metagraph() -> Result<()> {
        let namespaces = discover(SOURCE, &catalog())?;
        ensure!(
            namespaces
                == vec![Namespace {
                    accessor: "subnets".into(),
                    class: "Subnets".into(),
                    category: "Subnets".into()
                }]
        );
        let output = render(&namespaces, "source-commit");
        ensure!(output.contains("fn subnets(") && !output.contains("fn metagraph("));
        ensure!(!output.contains("spec_version"));
        Ok(())
    }

    #[test]
    fn rejects_category_prefix_drift_aliases_and_rebindings() -> Result<()> {
        for changed in [
            json!([{"category":"Unknown","python":""}]),
            json!([{"category":"Subnets","python":"sub.chain.token_symbols(...)"}]),
        ] {
            ensure!(discover(SOURCE, &changed).is_err());
        }
        for changed in [
            SOURCE.replace("{'subnets': Subnets}", "{'subnets': Missing}"),
            SOURCE.replace(
                "_category = 'Subnets'",
                "_category = 'Subnets'\n    _category = 'Other'",
            ),
            format!("{SOURCE}NAMESPACES['extra'] = Subnets\n"),
            SOURCE.replace(
                "NAMESPACES: dict",
                "def Subnets():\n    pass\nNAMESPACES: dict",
            ),
            SOURCE.replace(
                "_category = 'Subnets'",
                "_category = 'Subnets'\n    def _category(self):\n        return 'Other'",
            ),
            SOURCE.replace(
                "_category = 'Subnets'",
                "_category = 'Subnets'\n    _category += 'Other'",
            ),
            SOURCE.replace(
                "_category = 'Subnets'",
                "_category = 'Subnets'\n    if True:\n        _category = 'Other'",
            ),
        ] {
            ensure!(discover(&changed, &catalog()).is_err());
        }
        Ok(())
    }

    #[test]
    fn every_pinned_catalog_category_has_exactly_one_namespace() -> Result<()> {
        let root = super::super::root()?;
        let pins = super::super::read_document(&root, "upstream.toml")?;
        let source = String::from_utf8(input(&root, &pins, PYTHON)?)?;
        let catalog = serde_json::from_slice(&input(&root, &pins, CATALOG)?)?;
        let namespaces = discover(&source, &catalog)?;
        ensure!(namespaces.len() == 14);
        let output = super::super::format_rust(&render(&namespaces, "commit"))?;
        ensure!(output.contains("fn subnets(") && output.contains("fn balances("));
        Ok(())
    }
}
