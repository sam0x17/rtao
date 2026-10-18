//! Generate the downloadable book references, catalogs and LLM text.

mod examples;
mod extra;
mod reference;

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail, ensure};
use rtao_catalog::{INTENTS, IntentSpec, ParamSpec, READS};
use serde_json::Value;
use toml_edit::DocumentMut;

fn escaped(value: &str) -> String {
    value.replace('|', "\\|").replace(['\n', '\r'], " ")
}

fn table_text(value: &str) -> String {
    let mut text = String::new();
    for character in value.chars() {
        text.push_str(match character {
            '&' => "&amp;",
            '<' => "&lt;",
            '>' => "&gt;",
            '|' => "&#124;",
            '\\' => "&#92;",
            '`' => "&#96;",
            '*' => "&#42;",
            '_' => "&#95;",
            '[' => "&#91;",
            ']' => "&#93;",
            '~' => "&#126;",
            '\n' => "<br>",
            '\r' => continue,
            _ => {
                text.push(character);
                continue;
            }
        });
    }
    text
}

fn code(value: &str) -> String {
    format!("<code>{}</code>", table_text(value))
}

struct BookChapter {
    title: String,
    path: String,
    depth: usize,
}

fn book_chapters(summary: &str) -> Result<Vec<BookChapter>> {
    let mut chapters = Vec::new();
    for line in summary.lines() {
        let trimmed = line.trim_start();
        let Some(link) = trimmed.strip_prefix("- [") else {
            continue;
        };
        let (title, target) = link.split_once("](").context("book chapter link")?;
        let target = target.strip_suffix(')').context("book chapter target")?;
        if target.starts_with("https://") || target.starts_with("http://") {
            continue;
        }
        let path = target.split('#').next().context("book chapter path")?;
        ensure!(
            !path.is_empty()
                && Path::new(path)
                    .components()
                    .all(|part| matches!(part, std::path::Component::Normal(_)))
                && path.ends_with(".md"),
            "book chapter must name a relative Markdown file: {target}"
        );
        chapters.push(BookChapter {
            title: title.to_owned(),
            path: path.to_owned(),
            depth: (line.len() - trimmed.len()) / 2,
        });
    }
    ensure!(!chapters.is_empty(), "book summary contains no chapters");
    Ok(chapters)
}

fn append_book(
    summary: &str,
    outputs: &BTreeMap<String, String>,
    mut authored: impl FnMut(&str) -> Result<String>,
) -> Result<String> {
    let chapters = book_chapters(summary)?;
    let mut text = String::new();
    let mut included = BTreeSet::new();
    for path in
        std::iter::once("README.md").chain(chapters.iter().map(|chapter| chapter.path.as_str()))
    {
        if included.insert(path.to_owned()) {
            let chapter = match outputs.get(path) {
                Some(generated) => generated.clone(),
                None => authored(path)?,
            };
            text.push_str(&format!("## {path}\n\n{chapter}\n\n"));
        }
    }
    for (path, chapter) in outputs {
        if path.ends_with(".md") && path != "SUMMARY.md" && included.insert(path.clone()) {
            text.push_str(&format!("## {path}\n\n{chapter}\n\n"));
        }
    }
    Ok(text)
}

fn pending_intents(source: &str) -> Result<BTreeSet<String>> {
    pending_entries(source, "intents", INTENTS.iter().map(|spec| spec.name))
}

fn pending_entries<'a>(
    source: &str,
    kind: &str,
    names: impl Iterator<Item = &'a str>,
) -> Result<BTreeSet<String>> {
    let document: DocumentMut = source.parse()?;
    let known: BTreeSet<_> = names.collect();
    let mut pending = BTreeSet::new();
    for item in document[kind].as_array().context("pending entries")? {
        let name = item.as_str().context("pending entry must be a string")?;
        ensure!(known.contains(name), "unknown pending {kind} entry {name}");
        ensure!(
            pending.insert(name.to_owned()),
            "duplicate pending {kind} entry {name}"
        );
    }
    Ok(pending)
}

fn default_text(field: &ParamSpec) -> Result<String> {
    let Some(default) = field.default_json else {
        return Ok("—".into());
    };
    // JSON keeps null, false, zero, empty strings and fresh collections distinct.
    let default: Value = serde_json::from_str(default)?;
    let mut text = code(&serde_json::to_string(&default)?);
    if field.default_factory {
        text.push_str(" (fresh factory)");
    }
    Ok(text)
}

fn constructor_fields(spec: &IntentSpec, schema: &Value) -> Result<String> {
    let properties = schema["properties"]
        .as_object()
        .context("intent schema properties")?;
    let fields: BTreeSet<_> = spec.fields.iter().map(|field| field.name).collect();
    ensure!(
        fields == properties.keys().map(String::as_str).collect(),
        "{} constructor/schema fields disagree",
        spec.name
    );
    let required: BTreeSet<_> = schema
        .get("required")
        .map(|value| {
            value
                .as_array()
                .context("required fields must be an array")?
                .iter()
                .map(|value| value.as_str().context("required field must be a string"))
                .collect::<Result<BTreeSet<_>>>()
        })
        .transpose()?
        .unwrap_or_default();
    ensure!(
        required
            == spec
                .fields
                .iter()
                .filter(|field| field.required)
                .map(|field| field.name)
                .collect(),
        "{} constructor/schema required fields disagree",
        spec.name
    );
    if spec.fields.is_empty() {
        return Ok("No constructor fields.\n".into());
    }
    let mut text = "| Field | Python type | Required | Default (JSON) | Description |\n|---|---|---|---|---|\n".to_owned();
    for field in spec.fields {
        text.push_str(&format!(
            "| {} | {} | {} | {} | {} |\n",
            code(field.name),
            code(field.annotation.unwrap_or("undeclared")),
            if field.required { "yes" } else { "no" },
            default_text(field)?,
            table_text(
                properties[field.name]
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or("")
            ),
        ));
    }
    Ok(text)
}

fn schema_block(schema: &Value) -> Result<String> {
    let schema = serde_json::to_string_pretty(schema)?;
    // Catalog strings may contain Markdown fences; a longer fence keeps
    // every schema byte literal rather than turning it into book markup.
    let fence = "`".repeat(
        3.max(
            schema
                .split(|character| character != '`')
                .map(str::len)
                .max()
                .unwrap_or_default()
                + 1,
        ),
    );
    Ok(format!("{fence}json\n{schema}\n{fence}\n"))
}

fn transaction_reference(pending: &BTreeSet<String>) -> Result<String> {
    let mut text = format!(
        "# Transaction reference\n\nAll {} operations from the authenticated Python catalog are indexed below. Constructor fields, defaults, descriptions and schemas retain the pinned Python shape. Rust amounts use checked native u64 rao; see the compatibility record for deliberate differences.\n\nImplementation status comes from the source pending register, independently of the features compiled into the documentation generator. An implemented source entry is available through the CLI's generated transaction family; it does not assert successful hosted acceptance, chain interoperability or device coverage. Pending entries have no executable command. Required means required by the Python constructor; the CLI may prompt or resolve documented wallet defaults.\n\n| Operation | Summary | Source status | CLI |\n|---|---|---|---|\n",
        INTENTS.len()
    );
    for spec in &INTENTS {
        let implemented = !pending.contains(spec.name);
        text.push_str(&format!(
            "| [{}](#tx-{}) | {} | {} | {} |\n",
            code(spec.name),
            spec.name.replace('_', "-"),
            table_text(spec.summary),
            if implemented {
                "implemented"
            } else {
                "pending behavior port"
            },
            if implemented {
                code(&format!("rtao tx {}", spec.name.replace('_', "-")))
            } else {
                "unavailable".into()
            },
        ));
    }
    for spec in &INTENTS {
        let catalog: Value = serde_json::from_str(spec.catalog_json)?;
        let schema = catalog.get("input_schema").context("intent input schema")?;
        text.push_str(&format!(
            "\n<a id=\"tx-{}\"></a>\n\n## {}\n\n{}\n\n",
            spec.name.replace('_', "-"),
            spec.name,
            spec.description
        ));
        text.push_str(if pending.contains(spec.name) {
            "**Status:** pending behavior port; unavailable in the CLI.\n\n"
        } else {
            "**Status:** implemented source; hosted acceptance is recorded separately.\n\n"
        });
        if !pending.contains(spec.name) {
            text.push_str(&format!(
                "**CLI:** {}\n\n",
                code(&format!("rtao tx {}", spec.name.replace('_', "-")))
            ));
        }
        let calls = spec
            .wraps
            .iter()
            .map(|(pallet, call)| code(&format!("{pallet}.{call}")))
            .collect::<Vec<_>>()
            .join(", ");
        text.push_str(&format!(
            "| Contract | Value |\n|---|---|\n| Python class | {} |\n| Signing role | {} |\n| Dispatch origin | {} |\n| MEV shield default | {} |\n| MEV shield required | {} |\n| Verification read | {} |\n| Catalog calls | {} |\n| Fields accepting `all` | {} |\n\n",
            code(spec.python_class), code(spec.signer), code(spec.origin), spec.mev_shield_default, spec.mev_shield_required,
            spec.verify.map_or_else(|| "none".into(), code), calls,
            if spec.all_amount_fields.is_empty() { "none".into() } else { spec.all_amount_fields.iter().map(|name| code(name)).collect::<Vec<_>>().join(", ") },
        ));
        text.push_str("**Constructor fields**\n\n");
        text.push_str(&constructor_fields(spec, schema)?);
        text.push_str("\n**Input schema**\n\n");
        text.push_str(&schema_block(schema)?);
    }
    Ok(text)
}

fn content(root: &Path) -> Result<BTreeMap<String, String>> {
    let map: DocumentMut = fs::read_to_string(root.join("parity/map.toml"))?.parse()?;
    let rows = map
        .get("file")
        .and_then(toml_edit::Item::as_array_of_tables)
        .context("parity map has no file rows")?;
    let mut index = "# Python and Rust API index\n\nThe parity map is the file-level migration table for the pinned Python source and watched inputs. A pending row still needs source reconciliation; its Rust home is prospective. A reconciled row records a reviewed mapping or classification, including upstream-only producers. Source review does not certify operation availability or executed parity. Consult the generated transaction/query references and Rust API docs for available APIs.\n\n| Python source | Rust home | Kind | Source review | Milestone |\n|---|---|---|---|---|\n".to_owned();
    for row in rows {
        let py = row
            .get("py")
            .and_then(toml_edit::Item::as_str)
            .context("parity source")?;
        let rs = row
            .get("rs")
            .and_then(toml_edit::Item::as_array)
            .context("parity Rust homes")?
            .iter()
            .map(|item| item.as_str().context("Rust home is not a string"))
            .collect::<Result<Vec<_>>>()?
            .join(", ");
        let reconciled = row
            .get("synced_blob")
            .and_then(toml_edit::Item::as_str)
            .context("parity source reconciliation")?
            != "pending";
        index.push_str(&format!(
            "| `{}` | {} | {} | {} | {} |\n",
            escaped(py),
            escaped(&rs),
            escaped(
                row.get("kind")
                    .and_then(toml_edit::Item::as_str)
                    .context("parity kind")?
            ),
            if reconciled { "reconciled" } else { "pending" },
            escaped(
                row.get("milestone")
                    .and_then(toml_edit::Item::as_str)
                    .context("parity milestone")?
            )
        ));
    }
    let examples = examples::pages(root)?;
    let pending_source = fs::read_to_string(root.join("parity/pending.toml"))?;
    let pending = pending_intents(&pending_source)?;
    let pending_reads =
        pending_entries(&pending_source, "reads", READS.iter().map(|spec| spec.name))?;
    let transactions = transaction_reference(&pending)?;
    let mut outputs = reference::pages(root, &pending, &pending_reads)?;
    for (path, text) in &examples {
        ensure!(
            outputs.insert(path.clone(), text.clone()).is_none(),
            "duplicate compiled-example page {path}"
        );
    }
    for (path, text) in extra::pages()? {
        ensure!(
            outputs.insert(path.clone(), text).is_none(),
            "duplicate documentation path {path}"
        );
    }
    for (path, text) in reference::migration_pages(root)? {
        ensure!(
            outputs.insert(path.clone(), text).is_none(),
            "duplicate migration documentation path {path}"
        );
    }
    outputs.insert("api-index.md".into(), index);
    outputs.insert("tx-reference.md".into(), transactions);
    outputs.insert(
        "compat.md".into(),
        fs::read_to_string(root.join("COMPAT.md"))?,
    );
    let card = reference::agent_card(root, &outputs)?;
    outputs.insert("agent-card.md".into(), card);
    reference::add_indexes(root, &mut outputs)?;
    let mut llms = "# rtao\n\nRust SDK for Bittensor. The downloadable book includes rustdoc in `api/`. This text records source documentation and parity mappings; validation evidence is recorded separately.\n\n".to_owned();
    llms.push_str(&append_book(
        outputs
            .get("SUMMARY.md")
            .context("generated book summary")?,
        &outputs,
        |path| {
            fs::read_to_string(root.join("book/src").join(path))
                .with_context(|| format!("read book chapter {path}"))
        },
    )?);
    llms.push_str("\n## Rust API surface\n\nThe complete rustdoc tree is included in `api/` in the book archive. The following item signatures come from this source checkout.\n\n");
    // Rustdoc JSON is unstable. Walk the parsed source AST instead, keeping
    // module and impl context so signatures remain useful without inventing
    // documentation for generated or future items.
    let crates = root.join("crates");
    let mut crate_dirs = fs::read_dir(&crates)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    crate_dirs.sort();
    for dir in crate_dirs {
        if !dir.is_dir() {
            continue;
        }
        let mut files = Vec::new();
        source_files(&dir.join("src"), &mut files)?;
        files.sort();
        for file in files {
            let ast = syn::parse_file(&fs::read_to_string(&file)?)
                .with_context(|| format!("parse documentation source {}", file.display()))?;
            let relative = file.strip_prefix(root)?;
            llms.push_str(&format!("### {}\n\n", relative.display()));
            signatures(&ast.items, &mut llms, "");
            llms.push('\n');
        }
    }
    let llms_index = reference::llms_index(&outputs)?;
    outputs.insert("llms.txt".into(), llms_index.clone());
    outputs.insert("llms-full.txt".into(), format!("{llms_index}\n{llms}"));
    Ok(outputs)
}

fn source_files(dir: &Path, output: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        let kind = fs::symlink_metadata(&path)?.file_type();
        ensure!(
            !kind.is_symlink(),
            "documentation source contains a symlink"
        );
        if kind.is_dir() {
            source_files(&path, output)?;
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            output.push(path);
        }
    }
    Ok(())
}

fn public(visibility: &syn::Visibility) -> bool {
    matches!(visibility, syn::Visibility::Public(_))
}

fn signatures(items: &[syn::Item], text: &mut String, context: &str) {
    use quote::ToTokens;
    for item in items {
        let mut signature = None;
        let mut attributes: &[syn::Attribute] = &[];
        match item {
            syn::Item::Fn(item) if public(&item.vis) => {
                signature = Some(item.sig.to_token_stream().to_string());
                attributes = &item.attrs;
            }
            syn::Item::Struct(item) if public(&item.vis) => {
                signature = Some(format!(
                    "struct {}{}",
                    item.ident,
                    item.generics.to_token_stream()
                ));
                attributes = &item.attrs;
            }
            syn::Item::Enum(item) if public(&item.vis) => {
                signature = Some(format!(
                    "enum {}{}",
                    item.ident,
                    item.generics.to_token_stream()
                ));
                attributes = &item.attrs;
            }
            syn::Item::Trait(item) if public(&item.vis) => {
                signature = Some(format!(
                    "trait {}{}",
                    item.ident,
                    item.generics.to_token_stream()
                ));
                attributes = &item.attrs;
                text.push_str(&format!("Trait `{}`:\n", item.ident));
                for member in &item.items {
                    if let syn::TraitItem::Fn(method) = member {
                        text.push_str(&format!("- `{}`\n", method.sig.to_token_stream()));
                    }
                }
            }
            syn::Item::Type(item) if public(&item.vis) => {
                signature = Some(format!(
                    "type {} = {}",
                    item.ident,
                    item.ty.to_token_stream()
                ));
                attributes = &item.attrs;
            }
            syn::Item::Const(item) if public(&item.vis) => {
                signature = Some(format!(
                    "const {}: {}",
                    item.ident,
                    item.ty.to_token_stream()
                ));
                attributes = &item.attrs;
            }
            syn::Item::Use(item) if public(&item.vis) => {
                signature = Some(format!("use {}", item.tree.to_token_stream()));
                attributes = &item.attrs;
            }
            syn::Item::Impl(item) => {
                let owner = item.self_ty.to_token_stream().to_string();
                for member in &item.items {
                    if let syn::ImplItem::Fn(method) = member
                        && (public(&method.vis) || item.trait_.is_some())
                    {
                        text.push_str(&format!("- `{owner}::{}`\n", method.sig.to_token_stream()));
                    }
                }
            }
            syn::Item::Mod(item) if public(&item.vis) => {
                if let Some((_, items)) = &item.content {
                    signatures(items, text, &format!("{context}{}::", item.ident));
                }
            }
            _ => {}
        }
        if let Some(signature) = signature {
            text.push_str(&format!("- `{context}{signature}`\n"));
            for attribute in attributes {
                if !attribute.path().is_ident("doc") {
                    continue;
                }
                if let syn::Meta::NameValue(value) = &attribute.meta
                    && let syn::Expr::Lit(literal) = &value.value
                    && let syn::Lit::Str(line) = &literal.lit
                {
                    text.push_str(&format!("  {}\n", line.value().trim()));
                }
            }
        }
    }
}

pub(crate) fn run(args: Vec<String>) -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .context("workspace root")?;
    let mut out = None;
    let mut check = false;
    let mut llms = false;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--out" if out.is_none() => {
                out = Some(PathBuf::from(
                    args.next()
                        .context("--out requires an existing directory")?,
                ))
            }
            "--llms" if !llms => llms = true,
            "--check" if !check => check = true,
            _ => bail!("usage: cargo xtask docs --out <directory> [--llms] [--check]"),
        }
    }
    let out = out.context("--out is required; put generated docs under the recorded task root")?;
    ensure!(
        out.is_dir() && !fs::symlink_metadata(&out)?.file_type().is_symlink(),
        "docs output must be an existing regular directory"
    );
    let mut outputs = content(root)?;
    if !llms {
        outputs.remove("llms.txt");
        outputs.remove("llms-full.txt");
    }
    reference::write_outputs(&out, outputs, check)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn llm_book_follows_summary_includes_authored_guides_and_uses_each_overlay_once() -> Result<()>
    {
        let summary = "# Summary\n\n- [Start](getting-started.md)\n- [CLI](cli.md)\n- [Signers](signers.md)\n- [Ledger](ledger.md)\n- [EVM](evm.md)\n  - [Start again](getting-started.md)\n";
        let outputs = BTreeMap::from([
            ("README.md".into(), "compiled readme".into()),
            (
                "getting-started.md".into(),
                "exported example overlay".into(),
            ),
            ("SUMMARY.md".into(), summary.into()),
            ("aliases.md".into(), "generated aliases".into()),
        ]);
        let mut read = Vec::new();
        let full = append_book(summary, &outputs, |path| {
            read.push(path.to_owned());
            Ok(format!("authored {path}"))
        })?;
        assert_eq!(read, ["cli.md", "signers.md", "ledger.md", "evm.md"]);
        assert_eq!(full.matches("exported example overlay").count(), 1);
        assert_eq!(full.matches("compiled readme").count(), 1);
        assert!(full.contains("generated aliases"));
        assert!(!full.contains("## SUMMARY.md"));
        assert!(book_chapters("- [Escape](../outside.md)").is_err());
        assert!(book_chapters("- [Absolute](/outside.md)").is_err());
        Ok(())
    }

    #[test]
    fn transaction_reference_covers_exact_catalog_and_does_not_advertise_pending_commands()
    -> Result<()> {
        let pending = pending_intents("intents = ['add_collateral']")?;
        let reference = transaction_reference(&pending)?;
        let headings: Vec<_> = reference
            .lines()
            .filter_map(|line| line.strip_prefix("## "))
            .collect();
        assert_eq!(headings.len(), crate::extract::INTENT_COUNT);
        assert_eq!(
            headings.into_iter().collect::<BTreeSet<_>>(),
            INTENTS.iter().map(|spec| spec.name).collect()
        );
        for spec in &INTENTS {
            assert!(reference.contains(&format!(
                "<a id=\"tx-{}\"></a>",
                spec.name.replace('_', "-")
            )));
        }
        let collateral = reference
            .split("## add_collateral\n")
            .nth(1)
            .context("collateral section")?
            .split("\n## ")
            .next()
            .context("collateral body")?;
        assert!(collateral.contains("pending behavior port; unavailable in the CLI"));
        assert!(!reference.contains("rtao tx add-collateral"));
        assert!(reference.contains("rtao tx transfer"));
        assert!(reference.contains("implemented source; hosted acceptance is recorded separately"));
        assert!(pending_intents("intents = ['unknown_operation']").is_err());
        assert!(pending_intents("intents = ['transfer', 'transfer']").is_err());
        Ok(())
    }

    #[test]
    fn transaction_fields_keep_schema_text_and_distinct_json_defaults() -> Result<()> {
        const FIELDS: &[ParamSpec] = &[
            ParamSpec {
                name: "union",
                annotation: Some("int | str"),
                json_type: None,
                required: true,
                own_key: false,
                default_json: None,
                default_factory: false,
                provenance_json: "{}",
            },
            ParamSpec {
                name: "empty",
                annotation: Some("list"),
                json_type: None,
                required: false,
                own_key: false,
                default_json: Some("[]"),
                default_factory: true,
                provenance_json: "{}",
            },
        ];
        let spec = IntentSpec {
            fields: FIELDS,
            ..INTENTS[0]
        };
        let schema = serde_json::json!({
            "properties": {
                "union": {"description":"a|b\n`x` <tag> & *value*", "anyOf":[{"type":"integer"},{"type":"string"}]},
                "empty": {"type":"array"}
            },
            "required":["union"]
        });
        let text = constructor_fields(&spec, &schema)?;
        assert!(text.contains("| <code>union</code> | <code>int &#124; str</code> | yes | — |"));
        assert!(text.contains("a&#124;b<br>&#96;x&#96; &lt;tag&gt; &amp; &#42;value&#42;"));
        assert!(text.contains("| no | <code>&#91;&#93;</code> (fresh factory) |"));
        let literal_schema =
            serde_json::json!({"description":"``` | <tag> &", "enum":["",0,false,null]});
        let block = schema_block(&literal_schema)?;
        assert!(block.starts_with("````json\n"));
        assert!(block.ends_with("\n````\n"));
        let body = block
            .strip_prefix("````json\n")
            .and_then(|text| text.strip_suffix("\n````\n"))
            .context("schema fence")?;
        assert_eq!(serde_json::from_str::<Value>(body)?, literal_schema);
        for (json, expected) in [
            ("null", "<code>null</code>"),
            ("false", "<code>false</code>"),
            ("0", "<code>0</code>"),
            ("\"\"", "<code>\"\"</code>"),
            ("\"a\\\\b\"", "<code>\"a&#92;&#92;b\"</code>"),
            ("\"|`<&\"", "<code>\"&#124;&#96;&lt;&amp;\"</code>"),
        ] {
            let field = ParamSpec {
                default_json: Some(json),
                default_factory: false,
                ..FIELDS[1]
            };
            assert_eq!(default_text(&field)?, expected);
        }
        let mut missing = schema.clone();
        missing["properties"]
            .as_object_mut()
            .context("schema properties")?
            .remove("union");
        assert!(constructor_fields(&spec, &missing).is_err());
        let mut wrong_required = schema;
        wrong_required["required"] = serde_json::json!([]);
        assert!(constructor_fields(&spec, &wrong_required).is_err());
        Ok(())
    }

    #[test]
    fn llm_signatures_keep_owner_and_public_visibility() -> Result<()> {
        let ast = syn::parse_file(
            "pub struct Amount; impl Amount { pub fn checked_add(self, rhs: Self) -> Option<Self> { None } fn private_helper() {} } pub trait View { fn at(&self); } fn hidden() {}",
        )?;
        let mut result = String::new();
        signatures(&ast.items, &mut result, "");
        assert!(result.contains("struct Amount"));
        assert!(result.contains("Amount::fn checked_add"));
        assert!(result.contains("fn at"));
        assert!(!result.contains("private_helper"));
        assert!(!result.contains("hidden"));
        Ok(())
    }
}
