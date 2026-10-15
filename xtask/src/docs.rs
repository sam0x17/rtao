//! Generate the downloadable documentation's parity index and LLM text.

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail, ensure};
use toml_edit::DocumentMut;

fn escaped(value: &str) -> String {
    value.replace('|', "\\|").replace(['\n', '\r'], " ")
}

fn content(root: &Path) -> Result<(String, String)> {
    let map: DocumentMut = fs::read_to_string(root.join("parity/map.toml"))?.parse()?;
    let rows = map
        .get("file")
        .and_then(toml_edit::Item::as_array_of_tables)
        .context("parity map has no file rows")?;
    let mut index = "# Python and Rust API index\n\nThe parity map records implemented and planned Rust homes. A pending row is prospective; consult the Rust API docs for the shipped surface.\n\n| Python source | Rust home | Kind | Milestone |\n|---|---|---|---|\n".to_owned();
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
        index.push_str(&format!(
            "| `{}` | {} | {} | {} |\n",
            escaped(py),
            escaped(&rs),
            escaped(
                row.get("kind")
                    .and_then(toml_edit::Item::as_str)
                    .context("parity kind")?
            ),
            escaped(
                row.get("milestone")
                    .and_then(toml_edit::Item::as_str)
                    .context("parity milestone")?
            )
        ));
    }
    let mut llms = "# rtao\n\nRust SDK for Bittensor. The downloadable book includes rustdoc in `api/`. This text records source documentation and parity mappings; it is not a claim that pending milestones have shipped.\n\n".to_owned();
    for file in [
        "README.md",
        "COMPAT.md",
        "book/src/introduction.md",
        "book/src/getting-started.md",
        "book/src/validator.md",
        "book/src/miner.md",
        "book/src/releases.md",
    ] {
        llms.push_str(&format!(
            "## {file}\n\n{}\n\n",
            fs::read_to_string(root.join(file))?
        ));
    }
    llms.push_str(&index);
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
    Ok((index, llms))
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
    let (index, text) = content(root)?;
    for (name, bytes) in [("api-index.md", index), ("llms.txt", text)] {
        if name == "llms.txt" && !llms {
            continue;
        }
        let path = out.join(name);
        if path.exists() {
            ensure!(
                fs::symlink_metadata(&path)?.is_file(),
                "docs output must be a regular file"
            );
        }
        if check {
            ensure!(
                fs::read_to_string(&path)? == bytes,
                "{} is stale",
                path.display()
            );
        } else {
            fs::write(path, bytes)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
