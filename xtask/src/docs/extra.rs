//! Catalog-backed error and hyperparameter reference pages.

use std::collections::BTreeMap;

use anyhow::{Context, Result, ensure};
use rtao_catalog::{CHAIN_ERRORS, SEMANTIC_ERRORS, hyperparams::TABLE};
use serde_json::Value;
use toml_edit::DocumentMut;

use super::{code, table_text};

struct Upstream {
    raw_root: String,
    commit: String,
}

impl Upstream {
    fn pinned() -> Result<Self> {
        let pin: DocumentMut = include_str!("../../../upstream.toml").parse()?;
        let repository = pin["monorepo"]["url"]
            .as_str()
            .context("upstream repository URL")?
            .strip_prefix("https://github.com/")
            .context("upstream repository must use GitHub HTTPS")?
            .trim_end_matches(".git");
        ensure!(
            repository.split('/').count() == 2
                && repository.split('/').all(|part| !part.is_empty()
                    && part
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"_-.".contains(&byte))),
            "invalid upstream repository path"
        );
        let commit = pin["parity"]["commit"]
            .as_str()
            .context("upstream parity commit")?;
        ensure!(
            commit.len() == 40 && commit.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "upstream parity commit must be a full SHA"
        );
        Ok(Self {
            raw_root: format!("https://raw.githubusercontent.com/{repository}/{commit}"),
            commit: commit.to_owned(),
        })
    }

    fn source(&self, path: &str, item: &str) -> Result<String> {
        source_path(path)?;
        Ok(format!(
            "- {} — {} ([pinned source]({}/{path}), commit {}).\n",
            code(path),
            code(item),
            self.raw_root,
            code(&self.commit),
        ))
    }
}

fn identifier(value: &str) -> Result<()> {
    ensure!(
        !value.is_empty()
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_'),
        "invalid reference identifier {value:?}"
    );
    Ok(())
}

fn source_path(value: &str) -> Result<()> {
    ensure!(
        !value.is_empty()
            && value.split('/').all(|component| !component.is_empty()
                && component != "."
                && component != ".."
                && component
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))),
        "invalid upstream source path {value:?}"
    );
    Ok(())
}

fn raw_page(path: &str) -> Result<String> {
    let relative = path
        .strip_prefix("/llms.mdx/docs/")
        .context("catalog Markdown URL must be an upstream raw docs page")?;
    source_path(relative)?;
    Ok(format!("https://www.bittensor.com{path}"))
}

fn insert(pages: &mut BTreeMap<String, String>, path: String, text: String) -> Result<()> {
    ensure!(
        pages.insert(path.clone(), text).is_none(),
        "duplicate generated reference path {path}"
    );
    Ok(())
}

pub(super) fn pages() -> Result<BTreeMap<String, String>> {
    let upstream = Upstream::pinned()?;
    let mut pages = BTreeMap::new();
    for spec in &SEMANTIC_ERRORS {
        let token = spec.code.as_str();
        identifier(token)?;
        let mut text = format!(
            "# {token}\n\n{}\n\n**Remediation:** {}\n\n[Upstream raw page]({})\n\n## Rust\n\nThe semantic token is {}. Parse it with {}; a {} carries the classified code, message, optional chain name and remediation.\n\n- [ErrorCode](../api/rtao_catalog/enum.ErrorCode.html)\n- [ChainError](../api/rtao_tx/struct.ChainError.html)\n\n## Sources\n\n",
            spec.explanation,
            spec.remediation,
            raw_page(spec.markdown_url)?,
            code(token),
            code(&format!("rtao_catalog::ErrorCode::from_code(\"{token}\")")),
            code("rtao_tx::ChainError"),
        );
        text.push_str(&upstream.source(
            "sdk/python/bittensor/result.py",
            &format!("EXPLANATIONS, REMEDIATION ({token})"),
        )?);
        text.push_str(&upstream.source("sdk/python/bittensor/error_map.py", "ErrorCode")?);
        text.push_str("- `crates/rtao-catalog/src/errors.rs` — `ErrorCode`, `SEMANTIC_ERRORS`.\n- `crates/rtao-tx/src/error.rs` — `ChainError`.\n");
        insert(&mut pages, format!("errors/{token}.md"), text)?;
    }

    for spec in &CHAIN_ERRORS {
        identifier(spec.name)?;
        let token = spec.code.as_str();
        let pallets = spec
            .pallets
            .iter()
            .map(|pallet| code(pallet))
            .collect::<Vec<_>>()
            .join(", ");
        let mut text = format!(
            "# {}\n\n{}\n\n**Semantic code:** [{}](../{token}.md)\n\n**Pallets:** {}\n\n[Upstream raw page]({})\n\n## Rust\n\nThe exact chain name is {}. {} retains this name in its {} field and exposes the semantic code through {}. Use {} for the catalog description and {} for the applicable remedy; the remedy can depend on the message and exact name.\n\n- [ChainError](../../api/rtao_tx/struct.ChainError.html)\n- [Chain error catalog](../../api/rtao_catalog/static.CHAIN_ERRORS.html)\n\n## Sources\n\n",
            spec.name,
            spec.description,
            code(token),
            if pallets.is_empty() {
                "unspecified"
            } else {
                &pallets
            },
            raw_page(spec.markdown_url)?,
            code(spec.name),
            code("rtao_tx::ChainError"),
            code("name"),
            code("code"),
            code("ChainError::description()"),
            code("ChainError::remediation()"),
        );
        if let Some(sources) = spec.sources_json {
            let sources: Value = serde_json::from_str(sources)?;
            for source in sources
                .as_array()
                .context("chain sources must be an array")?
            {
                let path = source["path"].as_str().context("chain source path")?;
                let pallet = source["pallet"].as_str().context("chain source pallet")?;
                text.push_str(&upstream.source(path, &format!("{} ({pallet})", spec.name))?);
            }
        }
        // Some catalog errors have no runtime source record. Cite the catalog
        // rather than manufacturing a pallet path or a source line.
        text.push_str(&upstream.source(
            "website/apps/bittensor-website/public/catalog/errors.json",
            spec.name,
        )?);
        text.push_str("- `crates/rtao-catalog/src/errors.rs` — `CHAIN_ERRORS`.\n- `crates/rtao-tx/src/error.rs` — `ChainError::description`, `ChainError::remediation`.\n");
        insert(&mut pages, format!("errors/chain/{}.md", spec.name), text)?;
    }

    for (name, meta) in TABLE.entries {
        identifier(name)?;
        let (signed, bits) = TABLE.codec_of(name);
        let (minimum, maximum) = TABLE.raw_bounds(name);
        let storage = TABLE.storage.iter().find(|item| item.parameter == *name);
        let mut text = format!(
            "# {name}\n\n{}\n\n**Short description:** {}\n\n| Property | Value |\n|---|---|\n| Unit kind | {} |\n| Native codec | {} |\n| Inclusive raw minimum | {} |\n| Inclusive raw maximum | {} |\n| Accepted value forms | {} |\n",
            meta.doc,
            meta.short,
            code(TABLE.kind_of(name)),
            code(&if TABLE.kind_of(name) == "bool" && bits == 1 {
                "bool".to_owned()
            } else {
                format!("{}{bits}", if signed { "i" } else { "u" })
            }),
            code(&minimum.to_string()),
            code(&maximum.to_string()),
            table_text(&TABLE.value_forms(name)),
        );
        if let Some(storage) = storage {
            text.push_str(&format!(
                "| Storage | {} |\n| Python storage type identity | {} |\n| Pinned metadata type identity | {} |\n",
                code(&format!("{}.{}", storage.container, storage.name)),
                code(storage.source_ident),
                code(storage.metadata_ident),
            ));
        }
        text.push_str(&format!(
            "\n[Upstream raw source](https://www.bittensor.com/code/raw/sdk/python/bittensor/hyperparams.py)\n\n## Rust\n\n{} provides the authenticated semantics. {} resolves the unit kind, {} returns the effective inclusive range, and {} converts an input to a checked {}. Plain integers retain raw on-chain meaning; decimal human forms follow the accepted forms above. Unit kinds use metadata first; codec resolution honors explicit width overrides before metadata.\n\n- [HyperparamTable](../api/rtao_primitives/hyperparams/struct.HyperparamTable.html)\n- [RawInteger](../api/rtao_primitives/hyperparams/enum.RawInteger.html)\n\n## Sources\n\n",
            code("rtao_catalog::hyperparams::TABLE"),
            code(&format!("TABLE.kind_of(\"{name}\")")),
            code(&format!("TABLE.raw_bounds(\"{name}\")")),
            code(&format!("TABLE.to_raw(\"{name}\", input)")),
            code("RawInteger"),
        ));
        text.push_str(&upstream.source(
            "sdk/python/bittensor/hyperparams.py",
            &format!("HYPERPARAMS[\"{name}\"], kind_of, codec_of, raw_bounds, to_raw"),
        )?);
        if storage.is_some() {
            text.push_str(&upstream.source(
                "sdk/python/bittensor/hyperparams.py",
                &format!("STORAGE_ITEMS[\"{name}\"]"),
            )?);
        }
        text.push_str("- `crates/rtao-catalog/src/hyperparams.rs` — `TABLE`.\n- `crates/rtao-primitives/src/hyperparam_table.rs` — `HyperparamTable`, `RawInteger`.\n");
        insert(&mut pages, format!("hyperparameters/{name}.md"), text)?;
    }
    Ok(pages)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn every_catalog_entry_has_its_exact_page_and_untouched_explanation() -> Result<()> {
        let pages = pages()?;
        let expected: BTreeSet<_> = SEMANTIC_ERRORS
            .iter()
            .map(|spec| format!("errors/{}.md", spec.code.as_str()))
            .chain(
                CHAIN_ERRORS
                    .iter()
                    .map(|spec| format!("errors/chain/{}.md", spec.name)),
            )
            .chain(
                TABLE
                    .entries
                    .iter()
                    .map(|(name, _)| format!("hyperparameters/{name}.md")),
            )
            .collect();
        assert_eq!(pages.keys().cloned().collect::<BTreeSet<_>>(), expected);
        for spec in &SEMANTIC_ERRORS {
            let page = &pages[&format!("errors/{}.md", spec.code.as_str())];
            assert!(page.contains(spec.explanation));
            assert!(page.contains(spec.remediation));
            assert!(page.contains(spec.markdown_url));
        }
        for spec in &CHAIN_ERRORS {
            let page = &pages[&format!("errors/chain/{}.md", spec.name)];
            assert!(page.contains(spec.description));
            assert!(page.contains(spec.markdown_url));
            assert!(page.contains(&format!("../{}.md", spec.code.as_str())));
        }
        for (name, meta) in TABLE.entries {
            assert!(pages[&format!("hyperparameters/{name}.md")].contains(meta.doc));
        }
        for page in pages.values() {
            assert!(page.contains("\n## Rust\n"));
            assert!(
                !page.contains("#L"),
                "source links must survive line shifts"
            );
        }
        Ok(())
    }

    #[test]
    fn hyperparameters_show_semantic_bounds_and_real_storage_identities() -> Result<()> {
        let pages = pages()?;
        let tempo = &pages["hyperparameters/tempo.md"];
        assert!(tempo.contains("| Native codec | <code>u16</code> |"));
        assert!(tempo.contains("| Inclusive raw minimum | <code>360</code> |"));
        assert!(tempo.contains("| Inclusive raw maximum | <code>50400</code> |"));
        assert!(tempo.contains("SubtensorModule.Tempo"));
        let kappa = &pages["hyperparameters/kappa.md"];
        assert!(kappa.contains("| Unit kind | <code>u16</code> |"));
        assert!(kappa.contains("| Inclusive raw maximum | <code>65535</code> |"));
        assert!(kappa.contains("a 0..1 fraction (e.g. 0.5) or the raw integer (0..65535)"));
        // This composite parameter has hand semantics but no dedicated value.
        let epoch = &pages["hyperparameters/epoch_consensus.md"];
        assert!(!epoch.contains("| Storage |"));
        assert!(epoch.contains("| Inclusive raw maximum | <code>1</code> |"));
        Ok(())
    }

    #[test]
    fn source_and_page_paths_refuse_traversal_or_line_fragments() {
        for invalid in ["../error", "two/parts", "name#L42", ""] {
            assert!(identifier(invalid).is_err());
        }
        for invalid in ["../lib.rs", "/lib.rs", "a//lib.rs", "lib.rs#L1"] {
            assert!(source_path(invalid).is_err());
        }
        assert!(raw_page("/llms.mdx/docs/errors/not-found/content.md").is_ok());
        assert!(raw_page("https://other.example/page").is_err());
        assert!(raw_page("/llms.mdx/docs/../page.md").is_err());
    }
}
