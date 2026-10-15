// Derived from RaoFoundation/subtensor sdk/python/bittensor/_transport/runtime_api.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Frozen archive contracts from the authenticated Python declaration, without executing callbacks.

use std::{collections::BTreeSet, path::Path};

use anyhow::{Context, Result, bail, ensure};
use rustpython_parser::{Parse, ast};
use serde_json::json;
use toml_edit::DocumentMut;

use super::{MAX_CONTAINER_ITEMS, MAX_SOURCE_BYTES, Resolver, header, rust_string, rustfmt};

const SOURCE: &str = "sdk/python/bittensor/_transport/runtime_api.py";
const OUTPUT: &str = "crates/rtao-client/src/legacy_api_table.rs";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Accounts {
    None,
    One,
    Many,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Method {
    api: String,
    name: String,
    params: Vec<String>,
    output: String,
    accounts: Accounts,
    stake: bool,
}

fn string(expr: &ast::Expr) -> Result<&str> {
    if let ast::Expr::Constant(constant) = expr
        && let ast::Constant::Str(value) = &constant.value
        && !value.is_empty()
    {
        return Ok(value);
    }
    bail!("legacy contract value must be a nonempty literal string")
}

fn dictionary(expr: &ast::Expr) -> Result<Vec<(&str, &ast::Expr)>> {
    let ast::Expr::Dict(dict) = expr else {
        bail!("legacy contract must be a literal dictionary")
    };
    ensure!(
        dict.keys.len() <= MAX_CONTAINER_ITEMS && dict.keys.len() == dict.values.len(),
        "legacy dictionary exceeds bounds"
    );
    let mut seen = BTreeSet::new();
    dict.keys
        .iter()
        .zip(&dict.values)
        .map(|(key, value)| {
            let key = string(
                key.as_ref()
                    .context("legacy dictionary unpacking is unsupported")?,
            )?;
            ensure!(seen.insert(key), "duplicate legacy dictionary key {key}");
            Ok((key, value))
        })
        .collect()
}

fn field<'a>(fields: &[(&str, &'a ast::Expr)], name: &str) -> Result<&'a ast::Expr> {
    fields
        .iter()
        .find_map(|(key, value)| (*key == name).then_some(*value))
        .with_context(|| format!("legacy contract is missing {name}"))
}

fn registry(suite: &[ast::Stmt]) -> Result<&ast::Expr> {
    let mut found = None;
    for statement in suite {
        match statement {
            ast::Stmt::AnnAssign(assign) if matches!(assign.target.as_ref(), ast::Expr::Name(name) if name.id.as_str() == "LEGACY_RUNTIME_APIS") =>
            {
                ensure!(found.is_none(), "duplicate legacy registry assignment");
                found = Some(
                    assign
                        .value
                        .as_deref()
                        .context("legacy registry has no value")?,
                );
            }
            ast::Stmt::Import(_)
            | ast::Stmt::ImportFrom(_)
            | ast::Stmt::FunctionDef(_)
            | ast::Stmt::AsyncFunctionDef(_) => {}
            ast::Stmt::Expr(expr) if matches!(expr.value.as_ref(), ast::Expr::Constant(value) if matches!(&value.value, ast::Constant::Str(_))) =>
                {}
            _ => bail!("unsupported top-level statement in frozen registry module"),
        }
    }
    found.context("LEGACY_RUNTIME_APIS declaration absent")
}

fn parse(source: &str) -> Result<Vec<Method>> {
    ensure!(
        source.len() <= MAX_SOURCE_BYTES,
        "legacy registry source exceeds bound"
    );
    let suite = ast::Suite::parse(source, SOURCE).map_err(|error| anyhow::anyhow!("{error}"))?;
    let mut methods = Vec::new();
    for (api, entries) in dictionary(registry(&suite)?)? {
        for (name, definition) in dictionary(entries)? {
            let fields = dictionary(definition)?;
            ensure!(
                fields
                    .iter()
                    .all(|(key, _)| matches!(*key, "params" | "encoder" | "decoder")),
                "unknown legacy contract field"
            );
            let ast::Expr::List(inputs) = field(&fields, "params")? else {
                bail!("legacy params must be a literal list")
            };
            ensure!(
                inputs.elts.len() <= MAX_CONTAINER_ITEMS,
                "legacy params exceed bound"
            );
            let mut names = BTreeSet::new();
            let mut params = Vec::new();
            for input in &inputs.elts {
                let input = dictionary(input)?;
                ensure!(
                    input.len() == 2
                        && input.iter().all(|(key, _)| matches!(*key, "name" | "type")),
                    "legacy param must contain only name and type"
                );
                ensure!(
                    names.insert(string(field(&input, "name")?)?),
                    "duplicate legacy parameter name"
                );
                params.push(string(field(&input, "type")?)?.to_owned());
            }
            let encoder = fields
                .iter()
                .find_map(|(key, value)| (*key == "encoder").then_some(*value));
            let accounts = match encoder {
                None => Accounts::None,
                Some(ast::Expr::Name(encoder)) => match encoder.id.as_str() {
                    "_encode_account_vec_u8" => Accounts::One,
                    "_encode_accounts_vec_vec_u8" => Accounts::Many,
                    _ => bail!("unknown legacy account encoder"),
                },
                _ => bail!("legacy encoder must be a known function name"),
            };
            match accounts {
                Accounts::None => {}
                Accounts::One => ensure!(
                    params == ["Vec<u8>"],
                    "single-account encoder needs one byte vector"
                ),
                Accounts::Many => ensure!(
                    params == ["Vec<Vec<u8>>"],
                    "multi-account encoder needs nested byte vectors"
                ),
            }
            let (output, stake) = match field(&fields, "decoder")? {
                ast::Expr::Name(decoder) if decoder.id.as_str() == "_decode_stake_info_vec" => {
                    // This one custom decoder upgrades the frozen rows instead of resolving a named output.
                    ("Vec<StakeInfo>".to_owned(), true)
                }
                ast::Expr::Call(call)
                    if matches!(call.func.as_ref(), ast::Expr::Name(name) if name.id.as_str() == "_registry_decoder")
                        && call.args.len() == 1
                        && call.keywords.is_empty() =>
                {
                    (string(&call.args[0])?.to_owned(), false)
                }
                _ => bail!(
                    "legacy decoder must be the known stake decoder or literal registry factory"
                ),
            };
            methods.push(Method {
                api: api.into(),
                name: name.into(),
                params,
                output,
                accounts,
                stake,
            });
        }
    }
    ensure!(
        methods.len() == 13,
        "frozen registry must contain the reviewed thirteen methods"
    );
    Ok(methods)
}

fn render(methods: &[Method]) -> String {
    let mut source = String::from(
        "pub(crate) fn method(api: &str, name: &str) -> Option<LegacyMethod> {\nmatch (api, name) {\n",
    );
    for method in methods {
        let params = method
            .params
            .iter()
            .map(|value| rust_string(value))
            .collect::<Vec<_>>()
            .join(",");
        source.push_str(&format!("({},{}) => Some(LegacyMethod {{params: &[{params}], output: {}, accounts: Accounts::{:?}, stake: {} }}),\n", rust_string(&method.api), rust_string(&method.name), rust_string(&method.output), method.accounts, method.stake));
    }
    source.push_str("_ => None,\n}\n}\n");
    source
}

pub(super) fn outputs(root: &Path, pins: &DocumentMut) -> Result<Vec<(&'static str, Vec<u8>)>> {
    let resolver = Resolver::new(root)?;
    let source = resolver.source(SOURCE)?;
    let methods = parse(&source)?;
    let provenance = json!({"provenance":{"path":SOURCE,"commit":pins["parity"]["commit"].as_str().context("missing parity.commit")?}});
    let mut generated = header(&provenance)?;
    generated.push_str(&format!("// Source Git blob: {}\n// Included inside legacy_api; callback symbols are mapped to reviewed native policies.\n", resolver.tree.get(SOURCE).context("legacy source blob missing")?));
    generated.push_str(&render(&methods));
    Ok(vec![(OUTPUT, rustfmt(generated.into_bytes())?)])
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustpython_parser::ast::Ranged;
    use std::fs;

    const PINNED: &str =
        include_str!("../../vendor/upstream/src/sdk/python/bittensor/_transport/runtime_api.py");

    fn replace_expression(source: &str, expr: &ast::Expr, replacement: &str) -> String {
        let mut changed = source.to_owned();
        changed.replace_range(
            expr.range().start().to_usize()..expr.range().end().to_usize(),
            replacement,
        );
        changed
    }

    #[test]
    fn every_frozen_contract_is_derived_from_ordered_source_fields() -> Result<()> {
        let baseline = parse(PINNED)?;
        let suite =
            ast::Suite::parse(PINNED, SOURCE).map_err(|error| anyhow::anyhow!("{error}"))?;
        let mut visited = 0;
        for (api, entries) in dictionary(registry(&suite)?)? {
            for (name, definition) in dictionary(entries)? {
                let fields = dictionary(definition)?;
                let old = baseline
                    .iter()
                    .find(|method| method.api == api && method.name == name)
                    .context("method absent")?;
                // The changed type and factory must reach generated native code for every row,
                // including the row that formerly selected the custom stake decoder.
                let changed = parse(&replace_expression(
                    PINNED,
                    field(&fields, "decoder")?,
                    "_registry_decoder('ChangedHistoricalOutput')",
                ))?;
                let next = changed
                    .iter()
                    .find(|method| method.api == api && method.name == name)
                    .context("changed method absent")?;
                assert_eq!(next.output, "ChangedHistoricalOutput");
                assert!(!next.stake);
                assert_ne!(render(&changed), render(&baseline));
                let changed = replace_expression(
                    PINNED,
                    field(&fields, "params")?,
                    "[{'name':'second','type':'u32'},{'name':'first','type':'u64'}]",
                );
                if old.accounts == Accounts::None {
                    let changed = parse(&changed)?;
                    let next = changed
                        .iter()
                        .find(|method| method.api == api && method.name == name)
                        .context("changed method absent")?;
                    assert_eq!(next.params, ["u32", "u64"]);
                    assert_ne!(render(&changed), render(&baseline));
                } else {
                    assert!(
                        parse(&changed).is_err(),
                        "account encoder cannot silently change its wire contract"
                    );
                }
                assert!(
                    parse(&replace_expression(
                        PINNED,
                        field(&fields, "decoder")?,
                        "unknown_decoder"
                    ))
                    .is_err()
                );
                visited += 1;
            }
        }
        assert_eq!(visited, 13);
        Ok(())
    }

    #[test]
    fn encoder_and_custom_decoder_policies_are_explicit_and_fail_closed() -> Result<()> {
        let suite =
            ast::Suite::parse(PINNED, SOURCE).map_err(|error| anyhow::anyhow!("{error}"))?;
        let (_, entries) = dictionary(registry(&suite)?)?
            .into_iter()
            .next()
            .context("API absent")?;
        let (_, definition) = dictionary(entries)?
            .into_iter()
            .next()
            .context("method absent")?;
        for (row, accounts, stake) in [
            (
                "{'params': [], 'decoder': _registry_decoder('Changed')}",
                Accounts::None,
                false,
            ),
            (
                "{'params': [{'name':'a','type':'Vec<u8>'}], 'encoder': _encode_account_vec_u8, 'decoder': _decode_stake_info_vec}",
                Accounts::One,
                true,
            ),
            (
                "{'params': [{'name':'a','type':'Vec<Vec<u8>>'}], 'encoder': _encode_accounts_vec_vec_u8, 'decoder': _registry_decoder('Changed')}",
                Accounts::Many,
                false,
            ),
        ] {
            let methods = parse(&replace_expression(PINNED, definition, row))?;
            let first = methods.first().context("method absent")?;
            assert_eq!(first.accounts, accounts);
            assert_eq!(first.stake, stake);
        }
        for row in [
            "{'params': [], 'encoder': unknown_encoder, 'decoder': _registry_decoder('Changed')}",
            "{'params': [], 'encoder': _encode_account_vec_u8, 'decoder': _registry_decoder('Changed')}",
            "{'params': [], 'decoder': _registry_decoder(compute_type())}",
            "{'params': [], 'decoder': _registry_decoder('A'), 'decoder': _registry_decoder('B')}",
            "{'params': [], 'decoder': _registry_decoder('A'), 'new_policy': True}",
        ] {
            assert!(
                parse(&replace_expression(PINNED, definition, row)).is_err(),
                "{row}"
            );
        }
        for suffix in [
            "\nLEGACY_RUNTIME_APIS = {}\n",
            "\nLEGACY_RUNTIME_APIS.clear()\n",
        ] {
            assert!(parse(&format!("{PINNED}{suffix}")).is_err());
        }
        Ok(())
    }

    #[test]
    fn authenticated_registry_generates_the_entire_committed_lookup() -> Result<()> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .context("workspace root absent")?;
        let pins = fs::read_to_string(root.join("upstream.toml"))?.parse::<DocumentMut>()?;
        let generated = outputs(root, &pins)?;
        let (path, bytes) = generated.first().context("generated lookup absent")?;
        assert_eq!(*path, OUTPUT);
        assert_eq!(fs::read(root.join(path))?, *bytes);
        let mut resolver = Resolver::new(root)?;
        resolver.tree.insert(SOURCE.to_owned(), "0".repeat(40));
        assert!(
            resolver.source(SOURCE).is_err(),
            "modified source must not generate contracts"
        );
        Ok(())
    }
}
