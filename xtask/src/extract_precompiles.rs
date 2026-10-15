// Derived from RaoFoundation/subtensor sdk/python/bittensor/evm/precompiles.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Authenticated precompile declarations and the native tables that consume them.

use std::{collections::BTreeSet, fs, path::Path};

use anyhow::{Context, Result, bail, ensure};
use rustpython_parser::{Parse, ast};
use serde_json::Value;
use sha2::{Digest, Sha256};
use toml_edit::DocumentMut;

use super::{
    Binding, Bindings, Literal, MAX_CONTAINER_ITEMS, MAX_SOURCE_BYTES, Resolver, call_path, header,
    rust_string, rustfmt, value,
};

pub(super) const SOURCE: &str = "sdk/python/bittensor/evm/precompiles.py";
pub(super) const NAMES: &[&str] = &[
    "PRECOMPILES",
    "DEPRECATED_FUNCTIONS",
    "CALLER_ROLES",
    "_RAO_PARAM_NAMES",
    "STANDARD_PRECOMPILES",
];
const OUTPUT: &str = "crates/rtao-evm/src/precompile_catalog.rs";

fn validate_class(source: &str, bindings: &Bindings) -> Result<()> {
    ensure!(
        ["str", "int", "bool"]
            .iter()
            .all(|name| !bindings.contains_key(*name)),
        "Precompile annotation builtins must remain unshadowed"
    );
    ensure!(
        matches!(bindings.get("dataclass"), Some(Binding::Symbol(symbol)) if symbol == "dataclasses.dataclass"),
        "Precompile decorator must resolve to dataclasses.dataclass"
    );
    let suite = ast::Suite::parse(source, SOURCE).map_err(|e| anyhow::anyhow!("{e}"))?;
    let classes = suite
        .iter()
        .filter_map(|stmt| match stmt {
            ast::Stmt::ClassDef(class) if class.name.as_str() == "Precompile" => Some(class),
            _ => None,
        })
        .collect::<Vec<_>>();
    ensure!(classes.len() == 1, "Precompile must have one declaration");
    let class = classes[0];
    ensure!(
        class.bases.is_empty()
            && class.keywords.is_empty()
            && class.type_params.is_empty()
            && class.decorator_list.len() == 1,
        "Precompile class shape changed"
    );
    ensure!(
        matches!(&class.decorator_list[0], ast::Expr::Call(call)
        if call_path(&call.func).as_deref() == Some("dataclass") && call.args.is_empty()
            && call.keywords.len() == 1 && call.keywords[0].arg.as_ref().is_some_and(|name| name.as_str() == "frozen")
            && matches!(&call.keywords[0].value, ast::Expr::Constant(value) if value.value == ast::Constant::Bool(true))),
        "Precompile must remain a frozen literal dataclass"
    );
    let mut fields = Vec::new();
    let schema = [
        ("name", "str"),
        ("index", "int"),
        ("abi_file", "str"),
        ("description", "str"),
        ("deprecated", "bool"),
    ];
    let mut methods = BTreeSet::new();
    for stmt in &class.body {
        match stmt {
            ast::Stmt::AnnAssign(field) => {
                let ast::Expr::Name(name) = field.target.as_ref() else {
                    bail!("Precompile field must be a simple name")
                };
                let (expected_name, annotation) =
                    schema.get(fields.len()).context("extra Precompile field")?;
                // RustPython 0.4 drops parentheses before deriving simple.
                // CPython omits parenthesized names from dataclass annotations.
                ensure!(
                    field.simple
                        && source.get(field.range.start().to_usize()..name.range.end().to_usize())
                            == Some(*expected_name)
                        && name.id.as_str() == *expected_name
                        && matches!(field.annotation.as_ref(), ast::Expr::Name(actual) if actual.id.as_str() == *annotation),
                    "Precompile fields must use the ordinary builtin annotation schema"
                );
                if *expected_name == "deprecated" {
                    ensure!(
                        matches!(field.value.as_deref(), Some(ast::Expr::Constant(value)) if matches!(&value.value, ast::Constant::Bool(_))),
                        "Precompile deprecated default must be a literal boolean"
                    );
                } else {
                    ensure!(
                        field.value.is_none(),
                        "Precompile required fields may not have defaults or field modifiers"
                    );
                }
                fields.push(name.id.as_str());
            }
            ast::Stmt::FunctionDef(method) => {
                ensure!(
                    matches!(
                        method.name.as_str(),
                        "address" | "abi" | "functions" | "function"
                    ) && methods.insert(method.name.as_str()),
                    "Precompile may not override construction or field access"
                );
            }
            ast::Stmt::Expr(expr)
                if matches!(expr.value.as_ref(), ast::Expr::Constant(value)
                if matches!(&value.value, ast::Constant::Str(_))) => {}
            _ => bail!("unsupported statement in Precompile dataclass"),
        }
    }
    ensure!(
        fields == ["name", "index", "abi_file", "description", "deprecated"],
        "Precompile field schema/order changed"
    );
    Ok(())
}

/// Accept only the registry's bounded {p.name: p for p in (Precompile(...), ...)}.
/// Constructor/default resolution still uses the existing authenticated Resolver.
pub(super) fn keyed_records(
    resolver: &mut Resolver<'_>,
    expression: &ast::ExprDictComp,
    bindings: &Bindings,
    path: &str,
    depth: usize,
) -> Result<Literal> {
    ensure!(
        path == SOURCE && expression.generators.len() == 1,
        "only the precompile literal-record comprehension is accepted"
    );
    validate_class(&resolver.source(path)?, bindings)?;
    let generator = &expression.generators[0];
    let ast::Expr::Name(target) = &generator.target else {
        bail!("precompile comprehension target must be a name")
    };
    ensure!(
        !generator.is_async
            && generator.ifs.is_empty()
            && call_path(&expression.key).as_deref()
                == Some(format!("{}.name", target.id).as_str())
            && call_path(&expression.value).as_deref() == Some(target.id.as_str()),
        "precompile comprehension may not filter or transform records"
    );
    let ast::Expr::Tuple(items) = &generator.iter else {
        bail!("precompile comprehension requires a literal tuple")
    };
    ensure!(
        !items.elts.is_empty() && items.elts.len() <= MAX_CONTAINER_ITEMS,
        "precompile registry exceeds bounds"
    );
    let mut names = BTreeSet::new();
    let mut entries = Vec::new();
    for item in &items.elts {
        ensure!(
            matches!(item, ast::Expr::Call(call) if call_path(&call.func).as_deref() == Some("Precompile")),
            "precompile registry items must be literal Precompile constructors"
        );
        let record = resolver.eval(item, bindings, path, depth)?;
        let Literal::Resolved(fields) = &record else {
            bail!("precompile constructor did not resolve a record")
        };
        let name = fields["fields"]["name"]
            .as_str()
            .context("precompile name must be text")?;
        ensure!(
            !name.is_empty() && names.insert(name.to_owned()),
            "duplicate/empty precompile name"
        );
        entries.push((Literal::Str(name.to_owned()), record));
    }
    Ok(Literal::Dict(entries))
}

fn pairs(value: &Value) -> Result<Vec<(&str, &Value)>> {
    value["dict"]
        .as_array()
        .context("precompile table must be a dictionary")?
        .iter()
        .map(|pair| {
            let pair = pair
                .as_array()
                .context("precompile dictionary entry must be a pair")?;
            ensure!(
                pair.len() == 2,
                "precompile dictionary entry must be a pair"
            );
            Ok((
                pair[0]
                    .as_str()
                    .context("precompile table key must be text")?,
                &pair[1],
            ))
        })
        .collect()
}

fn string<'a>(record: &'a Value, field: &str) -> Result<&'a str> {
    record[field]
        .as_str()
        .with_context(|| format!("precompile field {field} must be text"))
}

// Stable native array order comes from the literal source declaration, not from
// Python frozenset iteration. Its membership must equal the resolved declaration.
fn rao_declaration_order(source: &str, table: &Value) -> Result<Vec<String>> {
    let suite = ast::Suite::parse(source, SOURCE).map_err(|e| anyhow::anyhow!("{e}"))?;
    let definitions = suite
        .iter()
        .filter_map(|stmt| match stmt {
            ast::Stmt::Assign(assign)
                if assign.targets.len() == 1
                    && call_path(&assign.targets[0]).as_deref() == Some("_RAO_PARAM_NAMES") =>
            {
                Some(assign.value.as_ref())
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    ensure!(
        definitions.len() == 1,
        "rao membership must have one literal declaration"
    );
    let ast::Expr::Call(call) = definitions[0] else {
        bail!("rao membership must use frozenset")
    };
    ensure!(
        call_path(&call.func).as_deref() == Some("frozenset")
            && call.args.len() == 1
            && call.keywords.is_empty(),
        "rao membership constructor changed"
    );
    let ast::Expr::Set(set) = &call.args[0] else {
        bail!("rao membership must use a literal set")
    };
    ensure!(
        set.elts.len() <= MAX_CONTAINER_ITEMS,
        "rao membership exceeds bound"
    );
    let mut names = Vec::new();
    for item in &set.elts {
        let ast::Expr::Constant(constant) = item else {
            bail!("rao name must be literal text")
        };
        let ast::Constant::Str(name) = &constant.value else {
            bail!("rao name must be text")
        };
        names.push(name.clone());
    }
    let resolved = value(table, "_RAO_PARAM_NAMES")?["frozenset"]
        .as_array()
        .context("rao membership must be a resolved frozenset")?
        .iter()
        .map(|value| value.as_str().context("rao name must be text"))
        .collect::<Result<BTreeSet<_>>>()?;
    let actual = names.iter().map(String::as_str).collect::<BTreeSet<_>>();
    ensure!(
        names.len() == actual.len() && actual == resolved,
        "rao declaration order/membership changed"
    );
    Ok(names)
}

fn render(table: &Value, rao: &[String]) -> Result<String> {
    let mut out = header(table)?;
    out.push_str("/// Native precompiles in authenticated Python declaration order.\npub const PRECOMPILES: &[Precompile] = &[\n");
    let mut indices = BTreeSet::new();
    for (key, record) in pairs(value(table, "PRECOMPILES")?)? {
        ensure!(
            record["constructor"].as_str() == Some("Precompile"),
            "catalog constructor changed"
        );
        let fields = record["fields"]
            .as_object()
            .context("precompile record fields absent")?;
        ensure!(
            fields.len() == 5
                && fields.keys().all(|key| matches!(
                    key.as_str(),
                    "name" | "index" | "abi_file" | "description" | "deprecated"
                )),
            "catalog field schema changed"
        );
        let fields = &record["fields"];
        ensure!(string(fields, "name")? == key, "catalog key/name mismatch");
        let index = u16::try_from(fields["index"].as_u64().context("index must be unsigned")?)?;
        ensure!(indices.insert(index), "duplicate precompile index");
        let file = string(fields, "abi_file")?;
        ensure!(
            file.ends_with(".json")
                && file
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
                && !file.contains(".."),
            "ABI filename must be a bounded local JSON basename"
        );
        let deprecated = fields["deprecated"]
            .as_bool()
            .context("deprecated must be boolean")?;
        out.push_str(&format!(
            "precompile!({}, {index}, {}, {}, {deprecated}),\n",
            rust_string(key),
            rust_string(file),
            rust_string(string(fields, "description")?)
        ));
    }
    out.push_str("];\n\n/// Pinned standard precompiles in Python declaration order.\npub const STANDARD_PRECOMPILES: &[(&str, u16)] = &[\n");
    for (name, index) in pairs(value(table, "STANDARD_PRECOMPILES")?)? {
        let index = u16::try_from(index.as_u64().context("standard index must be unsigned")?)?;
        out.push_str(&format!("({}, {index}),\n", rust_string(name)));
    }
    out.push_str("];\n\n/// Native rao parameter membership, in source literal order.\npub const RAO_PARAM_NAMES: &[&str] = &[\n");
    for name in rao {
        out.push_str(&format!("{},\n", rust_string(name)));
    }
    out.push_str("];\n\n");
    for (source, target) in [
        ("DEPRECATED_FUNCTIONS", "PRECOMPILE_FUNCTION_DEPRECATIONS"),
        ("CALLER_ROLES", "PRECOMPILE_CALLER_ROLES"),
    ] {
        out.push_str(&format!("const {target}: &[(&str, &str, &str)] = &[\n"));
        for (precompile, entries) in pairs(value(table, source)?)? {
            for (function, note) in pairs(entries)? {
                out.push_str(&format!(
                    "({}, {}, {}),\n",
                    rust_string(precompile),
                    rust_string(function),
                    rust_string(note.as_str().context("precompile hint must be text")?)
                ));
            }
        }
        out.push_str("];\n\n");
    }
    Ok(out)
}

fn authenticate_abis(root: &Path, pins: &DocumentMut, table: &Value) -> Result<()> {
    let resolver = Resolver::new(root)?;
    let mut functions = BTreeSet::new();
    for (name, record) in pairs(value(table, "PRECOMPILES")?)? {
        let file = string(&record["fields"], "abi_file")?;
        let source = format!("sdk/python/bittensor/evm/abi/{file}");
        let destination = format!("vendor/upstream/evm/abi/{file}");
        let artifact = &pins["artifacts"][source.as_str()];
        ensure!(
            artifact["to"].as_str() == Some(destination.as_str()),
            "ABI destination differs"
        );
        let path = root.join(&destination);
        ensure!(
            fs::metadata(&path)?.len() <= MAX_SOURCE_BYTES as u64,
            "ABI exceeds extraction bound"
        );
        let bytes = fs::read(path)?;
        ensure!(
            bytes.len() <= MAX_SOURCE_BYTES
                && artifact["sha256"].as_str()
                    == Some(format!("{:x}", Sha256::digest(&bytes)).as_str())
                && resolver.tree.get(&source) == Some(&super::git_blob(&bytes)),
            "ABI artifact/tree identity differs"
        );
        let abi: Value = serde_json::from_slice(&bytes)?;
        for entry in abi.as_array().context("ABI must be an array")? {
            if entry["type"] == "function" {
                functions.insert((name.to_owned(), string(entry, "name")?.to_owned()));
            }
        }
    }
    for source in ["DEPRECATED_FUNCTIONS", "CALLER_ROLES"] {
        for (precompile, entries) in pairs(value(table, source)?)? {
            for (function, _) in pairs(entries)? {
                ensure!(
                    functions.contains(&(precompile.to_owned(), function.to_owned())),
                    "catalog hint names an absent ABI function"
                );
            }
        }
    }
    Ok(())
}

pub(super) fn outputs(
    root: &Path,
    pins: &DocumentMut,
    tables: &Value,
) -> Result<Vec<(&'static str, Vec<u8>)>> {
    let table = &tables[SOURCE];
    let resolver = Resolver::new(root)?;
    let source = resolver.source(SOURCE)?;
    let rao = rao_declaration_order(&source, table)?;
    let output = render(table, &rao)?;
    authenticate_abis(root, pins, table)?;
    Ok(vec![(OUTPUT, rustfmt(output.into_bytes())?)])
}

#[cfg(test)]
mod tests {
    use super::super::document;
    use super::*;

    fn resolved(source: &str) -> Result<Value> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .context("workspace root")?;
        let pins: DocumentMut = fs::read_to_string(root.join("upstream.toml"))?.parse()?;
        let mut resolver = Resolver::new(root)?;
        resolver.sources.insert(SOURCE.into(), source.into());
        document(&mut resolver, &pins, SOURCE, NAMES)
    }

    #[test]
    fn source_mutations_drive_all_native_catalog_fields_and_hint_membership() -> Result<()> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .context("workspace root")?;
        let source = Resolver::new(root)?.source(SOURCE)?;
        let table = resolved(&source)?;
        ensure!(
            pairs(value(&table, "PRECOMPILES")?)?.len() == 21,
            "pinned catalog membership changed"
        );
        ensure!(
            pairs(value(&table, "STANDARD_PRECOMPILES")?)?.len() == 11,
            "pinned standard membership changed"
        );
        for (name, expected) in [("CALLER_ROLES", 27), ("DEPRECATED_FUNCTIONS", 2)] {
            let count = pairs(value(&table, name)?)?
                .into_iter()
                .map(|(_, inner)| pairs(inner).map(|pairs| pairs.len()))
                .collect::<Result<Vec<_>>>()?
                .into_iter()
                .sum::<usize>();
            ensure!(count == expected, "pinned metadata membership changed");
        }
        ensure!(
            rao_declaration_order(&source, &table)?.len() == 9,
            "pinned rao membership changed"
        );
        let baseline = render(&table, &rao_declaration_order(&source, &table)?)?;
        for (before, after) in [
            (
                "\"balance-transfer\",\n            2048",
                "\"renamed-transfer\",\n            2048",
            ),
            ("            2048,", "            2099,"),
            ("\"balanceTransfer.json\"", "\"changed.json\""),
            (
                "Transfer TAO from an EVM account to an ss58 account.",
                "Changed source description.",
            ),
            ("deprecated=True", "deprecated=False"),
            ("deprecated: bool = False", "deprecated: bool = True"),
            (
                "always returns 0 on chain; trust is no longer computed",
                "Changed source deprecation.",
            ),
            (
                "\"transferStakeFrom\": \"approved spender\"",
                "\"transferStakeFrom\": \"changed source role\"",
            ),
            ("\"amountRao\",", "\"newAmount\","),
            ("\"ecrecover\": 1,", "\"ecrecover\": 12,"),
        ] {
            ensure!(source.contains(before), "source mutation sentinel absent");
            let changed = source.replacen(before, after, 1);
            let changed_table = resolved(&changed)?;
            ensure!(
                render(
                    &changed_table,
                    &rao_declaration_order(&changed, &changed_table)?
                )? != baseline,
                "source mutation failed to reach the production renderer"
            );
        }
        let changed = source.replacen("deprecated: bool = False", "deprecated: bool = True", 1);
        ensure!(
            changed != source,
            "boolean default mutation sentinel absent"
        );
        let changed_table = resolved(&changed)?;
        ensure!(
            pairs(value(&table, "PRECOMPILES")?)?[0].1["fields"]["deprecated"] == false,
            "pinned first record default changed"
        );
        ensure!(
            pairs(value(&changed_table, "PRECOMPILES")?)?
                .iter()
                .all(|(_, record)| record["fields"]["deprecated"] == true),
            "literal default True must reach every omitted instance argument"
        );
        // Source dictionary order, rather than a sorted or separately maintained list.
        let changed = source.replace(
            "\"ecrecover\": 1,\n    \"sha256\": 2,",
            "\"sha256\": 2,\n    \"ecrecover\": 1,",
        );
        ensure!(changed != source, "standard order mutation sentinel absent");
        let changed_table = resolved(&changed)?;
        ensure!(
            render(
                &changed_table,
                &rao_declaration_order(&changed, &changed_table)?
            )? != baseline,
            "standard order mutation was ignored"
        );
        let first = source
            .find("        Precompile(")
            .context("first registry constructor")?;
        let second = first
            + 1
            + source[first + 1..]
                .find("        Precompile(")
                .context("second registry constructor")?;
        let third = second
            + 1
            + source[second + 1..]
                .find("        Precompile(")
                .context("third registry constructor")?;
        let changed = format!(
            "{}{}{}{}",
            &source[..first],
            &source[second..third],
            &source[first..second],
            &source[third..]
        );
        let changed_table = resolved(&changed)?;
        ensure!(
            pairs(value(&changed_table, "PRECOMPILES")?)?[0].0 == "staking",
            "registry source order was lost"
        );
        ensure!(
            render(
                &changed_table,
                &rao_declaration_order(&changed, &changed_table)?
            )? != baseline,
            "registry order mutation was ignored"
        );
        Ok(())
    }

    #[test]
    fn comprehension_refuses_filters_projection_changes_duplicates_and_custom_construction()
    -> Result<()> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .context("workspace root")?;
        let source = Resolver::new(root)?.source(SOURCE)?;
        for (before, after) in [
            ("p.name: p", "p.index: p"),
            ("p.name: p", "p.name: p.name"),
            ("    )\n}", "    ) if p\n}"),
            (
                "\"balance-transfer\",\n            2048",
                "\"staking\",\n            2048",
            ),
            (
                "    @property\n    def address",
                "    def __post_init__(self): pass\n\n    @property\n    def address",
            ),
            ("@dataclass(frozen=True)", "@dataclass(frozen=False)"),
        ] {
            ensure!(source.contains(before), "shape mutation sentinel absent");
            ensure!(
                resolved(&source.replacen(before, after, 1)).is_err(),
                "unsafe source shape accepted"
            );
        }
        Ok(())
    }

    #[test]
    fn constructor_semantics_refuse_pseudo_fields_modifiers_aliases_and_shadowed_builtins()
    -> Result<()> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .context("workspace root")?;
        let source = Resolver::new(root)?.source(SOURCE)?;
        for (before, after, imports) in [
            (
                "name: str",
                "name: InitVar[str]",
                "from dataclasses import InitVar",
            ),
            (
                "name: str",
                "name: dataclasses.InitVar[str]",
                "import dataclasses",
            ),
            (
                "deprecated: bool = False",
                "deprecated: typing.ClassVar[bool] = False",
                "import typing",
            ),
            (
                "deprecated: bool = False",
                "deprecated: ClassVar[bool] = False",
                "from typing import ClassVar",
            ),
            (
                "deprecated: bool = False",
                "deprecated: bool = field(default=False, init=False)",
                "from dataclasses import field",
            ),
            (
                "deprecated: bool = False",
                "deprecated: bool = field(default=False, kw_only=True)",
                "from dataclasses import field",
            ),
            (
                "name: str",
                "name: Alias",
                "from dataclasses import InitVar\nAlias = InitVar[str]",
            ),
            (
                "name: str",
                "name: str",
                "from dataclasses import InitVar\nstr = InitVar[str]",
            ),
            ("index: int", "index: int = 2048", ""),
            ("deprecated: bool = False", "deprecated: bool", ""),
            ("deprecated: bool = False", "deprecated: bool = 0", ""),
            ("name: str", "(name): str", ""),
            ("name: str", "((name)): str", ""),
            ("index: int", "(index): int", ""),
        ] {
            ensure!(
                source.contains(before) && source.contains("from typing import Any"),
                "constructor-semantic mutation sentinel absent"
            );
            let changed = source.replacen(before, after, 1).replacen(
                "from typing import Any",
                &format!("from typing import Any\n{imports}"),
                1,
            );
            ensure!(
                changed != source,
                "constructor-semantic mutation did not change source"
            );
            ensure!(
                resolved(&changed).is_err(),
                "unsupported constructor semantics accepted: {after:?}; imports: {imports:?}"
            );
        }
        Ok(())
    }
}
