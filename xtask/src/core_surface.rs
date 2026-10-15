//! Compile the curated upstream imports and the paths guarded by Clippy.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail, ensure};
use syn::{Item, UseTree, Visibility};
use toml_edit::DocumentMut;

fn use_paths(tree: &UseTree, prefix: &str, paths: &mut Vec<String>) -> Result<()> {
    match tree {
        UseTree::Path(path) => {
            let next = format!("{prefix}{}::", path.ident);
            use_paths(&path.tree, &next, paths)?;
        }
        UseTree::Name(name) => {
            paths.push(if name.ident == "self" {
                prefix.trim_end_matches("::").to_owned()
            } else {
                format!("{prefix}{}", name.ident)
            });
        }
        UseTree::Rename(rename) => paths.push(format!("{prefix}{}", rename.ident)),
        UseTree::Group(group) => {
            for item in &group.items {
                use_paths(item, prefix, paths)?;
            }
        }
        UseTree::Glob(_) => {
            bail!("curated core exports must name each item; a glob cannot be audited")
        }
    }
    Ok(())
}

/// Resolve a Rust module file, rejecting conditional or escaping paths that
/// these source guards cannot audit reliably.
pub(crate) fn module_file(
    module: &syn::ItemMod,
    directory: &Path,
    source_root: &Path,
) -> Result<PathBuf> {
    ensure!(
        !module
            .attrs
            .iter()
            .any(|attr| attr.path().is_ident("cfg_attr")),
        "conditional module attributes require an explicit audited module: {}",
        module.ident
    );
    let explicit: Vec<_> = module
        .attrs
        .iter()
        .filter(|attr| attr.path().is_ident("path"))
        .collect();
    ensure!(
        explicit.len() <= 1,
        "duplicate module path: {}",
        module.ident
    );
    let candidate = if let Some(attribute) = explicit.first() {
        let syn::Meta::NameValue(value) = &attribute.meta else {
            bail!("module path must be a string: {}", module.ident);
        };
        let syn::Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Str(path),
            ..
        }) = &value.value
        else {
            bail!("module path must be a string: {}", module.ident);
        };
        directory.join(path.value())
    } else {
        let flat = directory.join(format!("{}.rs", module.ident));
        let nested = directory.join(module.ident.to_string()).join("mod.rs");
        ensure!(
            flat.is_file() != nested.is_file(),
            "module {} needs exactly one source file at {} or {}",
            module.ident,
            flat.display(),
            nested.display()
        );
        if flat.is_file() { flat } else { nested }
    };
    let canonical_root = source_root.canonicalize()?;
    let canonical = candidate
        .canonicalize()
        .with_context(|| format!("module source {}", candidate.display()))?;
    ensure!(
        canonical.starts_with(&canonical_root),
        "module source escapes audited source root: {}",
        candidate.display()
    );
    for ancestor in candidate
        .ancestors()
        .take_while(|path| path.starts_with(source_root))
    {
        ensure!(
            !fs::symlink_metadata(ancestor)?.file_type().is_symlink(),
            "module source uses a symlink: {}",
            ancestor.display()
        );
    }
    Ok(canonical)
}

pub(crate) fn module_directory(source: &Path) -> Result<PathBuf> {
    let parent = source.parent().context("module source has no parent")?;
    Ok(
        if source
            .file_name()
            .is_some_and(|name| name == "lib.rs" || name == "main.rs" || name == "mod.rs")
        {
            parent.to_owned()
        } else {
            parent.join(source.file_stem().context("module source has no stem")?)
        },
    )
}

struct ExportSource<'a> {
    path: &'a Path,
    directory: PathBuf,
    inline: bool,
    root: &'a Path,
}

fn exports(
    items: &[Item],
    inherited_cfg: &[String],
    result: &mut BTreeSet<(Vec<String>, String)>,
    source: &ExportSource<'_>,
    active: &mut BTreeSet<PathBuf>,
) -> Result<()> {
    for item in items {
        let attributes = match item {
            Item::Mod(module) => &module.attrs,
            Item::Use(import) if matches!(import.vis, Visibility::Public(_)) => &import.attrs,
            _ => continue,
        };
        ensure!(
            !attributes
                .iter()
                .any(|attribute| attribute.path().is_ident("cfg_attr")),
            "conditional module/export attributes must be explicit before surface generation"
        );
        let mut cfg = inherited_cfg.to_vec();
        for attribute in attributes
            .iter()
            .filter(|attribute| attribute.path().is_ident("cfg"))
        {
            cfg.push(quote::quote!(#attribute).to_string());
        }
        if let Item::Mod(module) = item {
            if let Some((_, children)) = &module.content {
                exports(
                    children,
                    &cfg,
                    result,
                    &ExportSource {
                        path: source.path,
                        directory: source.directory.join(module.ident.to_string()),
                        inline: true,
                        root: source.root,
                    },
                    active,
                )?;
            } else {
                // At file scope #[path] is relative to that file; within an
                // inline module it is relative to the module directory.
                let base = if !source.inline
                    && module.attrs.iter().any(|attr| attr.path().is_ident("path"))
                {
                    source
                        .path
                        .parent()
                        .context("module source has no parent")?
                } else {
                    &source.directory
                };
                let child = module_file(module, base, source.root)?;
                ensure!(
                    active.insert(child.clone()),
                    "cyclic module source: {}",
                    child.display()
                );
                let syntax = syn::parse_file(&fs::read_to_string(&child)?)?;
                exports(
                    &syntax.items,
                    &cfg,
                    result,
                    &ExportSource {
                        path: &child,
                        directory: module_directory(&child)?,
                        inline: false,
                        root: source.root,
                    },
                    active,
                )?;
                active.remove(&child);
            }
        }
        if let Item::Use(import) = item {
            let mut paths = Vec::new();
            use_paths(&import.tree, "", &mut paths)?;
            for path in paths
                .into_iter()
                .filter(|path| path.starts_with("bittensor_core::"))
            {
                result.insert((cfg.clone(), path));
            }
        }
    }
    Ok(())
}

fn guard_signature(path: &str) -> Result<(&'static str, String)> {
    let signature = match path {
        "bittensor_core::client::Client::connect" => {
            "fn(String) -> Result<bittensor_core::client::Client, bittensor_core::CoreError>"
        }
        "bittensor_core::transaction::Executor::new" => {
            "fn(&'static bittensor_core::client::Client) -> bittensor_core::transaction::Executor<'static>"
        }
        "bittensor_core::transaction::Executor::with_policy" => {
            "fn(&'static bittensor_core::client::Client, bittensor_core::transaction::Policy) -> bittensor_core::transaction::Executor<'static>"
        }
        "bittensor_core::timelock::generate_commit_v2" => {
            "fn(Vec<u16>, Vec<u16>, u64, bittensor_core::timelock::epoch_schedule::EpochScheduleState, u64, f64, Vec<u8>) -> Result<(Vec<u8>, u64), bittensor_core::CoreError>"
        }
        "bittensor_core::timelock::decrypt" => {
            "fn(&[u8], bool) -> Result<Option<Vec<u8>>, bittensor_core::CoreError>"
        }
        "bittensor_core::timelock::get_round_info" => {
            "fn(Option<u64>) -> Result<bittensor_core::timelock::DrandResponse, bittensor_core::CoreError>"
        }
        "bittensor_core::timelock::get_reveal_round_signature" => {
            "fn(Option<u64>, bool) -> Result<Option<String>, bittensor_core::CoreError>"
        }
        "bittensor_core::timelock::epoch_schedule::predict_first_reveal_block" => {
            "fn(&bittensor_core::timelock::epoch_schedule::EpochScheduleState, u64) -> Result<u64, bittensor_core::timelock::epoch_schedule::EpochScheduleError>"
        }
        "bittensor_core::timelock::epoch_schedule::should_run_epoch" => {
            "fn(&bittensor_core::timelock::epoch_schedule::EpochScheduleState, u64) -> bool"
        }
        "bittensor_core::keyfiles::save_password_to_environment" => {
            "fn(&str, &str) -> Result<String, bittensor_core::CoreError>"
        }
        _ => bail!("no reviewed signature for Clippy guard {path}; add its compile probe"),
    };
    Ok((signature, path.to_owned()))
}

fn render(source: &Path, clippy: &str, commit: &str) -> Result<String> {
    ensure!(
        commit.len() == 40 && commit.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "invalid source commit"
    );
    let source = source.canonicalize()?;
    let syntax = syn::parse_file(&fs::read_to_string(&source)?)?;
    let mut paths = BTreeSet::new();
    exports(
        &syntax.items,
        &[],
        &mut paths,
        &ExportSource {
            path: &source,
            directory: module_directory(&source)?,
            inline: false,
            root: source.parent().context("core root source has no parent")?,
        },
        &mut BTreeSet::from([source.clone()]),
    )?;
    ensure!(
        !paths.is_empty(),
        "core exports are empty; refusing a vacuous surface check"
    );
    let config: DocumentMut = clippy.parse()?;
    let guards = config
        .get("disallowed-methods")
        .and_then(toml_edit::Item::as_array)
        .context("Clippy guards are missing")?;
    ensure!(
        guards.len() >= 10,
        "the ten M0 Clippy guards must all be present"
    );
    let mut generated = format!(
        "// Derived from RaoFoundation/subtensor sdk/bittensor-core/src/lib.rs @ {commit} (Apache-2.0); modified by rtao.\n//! Generated by cargo xtask core-surface.\n\n"
    );
    for (index, (cfg, path)) in paths.iter().enumerate() {
        for attribute in cfg {
            generated.push_str(attribute);
            generated.push('\n');
        }
        generated.push_str(&format!(
            "#[allow(unused_imports)]\nuse {path} as SurfaceItem{index};\n\n"
        ));
    }
    let mut seen = BTreeSet::new();
    for guard in guards {
        let table = guard
            .as_inline_table()
            .context("Clippy guards must be inline tables")?;
        let path = table
            .get("path")
            .and_then(toml_edit::Value::as_str)
            .context("guard has no path")?;
        ensure!(seen.insert(path), "duplicate Clippy guard {path}");
        let (signature, expression) = guard_signature(path)?;
        if table
            .get("allow-invalid")
            .and_then(toml_edit::Value::as_bool)
            == Some(true)
        {
            generated.push_str("#[cfg(feature = \"host\")]\n");
        }
        generated.push_str(&format!(
            "// The function pointer preserves the reviewed upstream signature.\n#[allow(clippy::disallowed_methods, clippy::type_complexity)]\nconst _: {signature} = {expression};\n\n"
        ));
    }
    // Parsing proves generation produced valid Rust before rustfmt or Cargo sees it.
    syn::parse_file(&generated)?;
    Ok(generated)
}

pub(crate) fn run(args: Vec<String>) -> Result<()> {
    ensure!(
        args.is_empty() || args == ["--check"],
        "usage: cargo xtask core-surface [--check]"
    );
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .context("workspace parent missing")?;
    let pins = crate::pin::read_document(&root.join("upstream.toml"))?;
    let commit = crate::core_canary::commit(&pins)?.unwrap_or(
        pins["core"]["commit"]
            .as_str()
            .context("missing core.commit")?,
    );
    let rendered = render(
        &root.join("crates/rtao-core/src/lib.rs"),
        &fs::read_to_string(root.join("clippy.toml"))?,
        commit,
    )?;
    let destination = root.join("crates/rtao-core/tests/core_surface.rs");
    if args.is_empty() {
        fs::write(&destination, rendered)?;
        let status = Command::new("rustfmt")
            .args(["--edition", "2024"])
            .arg(&destination)
            .status()?;
        ensure!(status.success(), "format generated core surface failed");
    } else {
        // Rustfmt formats a temporary input under the recorded task root.
        let temporary = std::env::var_os("RTAO_TEMP_ROOT")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| root.join("target"));
        fs::create_dir_all(&temporary)?;
        let scratch = temporary.join("core-surface-check.rs");
        fs::write(&scratch, rendered)?;
        let status = Command::new("rustfmt")
            .args(["--edition", "2024"])
            .arg(&scratch)
            .status()?;
        let check = fs::read(&scratch);
        fs::remove_file(scratch)?;
        ensure!(status.success(), "format surface check failed");
        ensure!(
            check? == fs::read(destination)?,
            "core surface is stale; run cargo xtask core-surface"
        );
    }
    let status = Command::new("cargo")
        .current_dir(root)
        .args([
            "test",
            "-p",
            "rtao-core",
            "--test",
            "core_surface",
            "--all-features",
            "--locked",
        ])
        .status()?;
    ensure!(status.success(), "core surface no longer compiles");
    println!("core surface imports and Clippy guard signatures verified");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inline_exports(source: &str) -> Result<BTreeSet<(Vec<String>, String)>> {
        let file = syn::parse_file(source)?;
        let mut paths = BTreeSet::new();
        exports(
            &file.items,
            &[],
            &mut paths,
            &ExportSource {
                path: Path::new("/unused/lib.rs"),
                directory: PathBuf::from("/unused"),
                inline: false,
                root: Path::new("/unused"),
            },
            &mut BTreeSet::new(),
        )?;
        Ok(paths)
    }

    #[test]
    fn exports_keep_feature_guards_and_expand_aliases_and_groups() -> Result<()> {
        let paths = inline_exports(
            "#[cfg(feature = \"host\")] pub mod keys { pub use bittensor_core::keys::{Keypair, verify as verifier}; } pub use bittensor_core::codec::value::Value;",
        )?;
        ensure!(paths.len() == 3);
        for (cfg, path) in paths {
            ensure!(cfg.is_empty() == path.contains("codec"));
        }
        Ok(())
    }

    #[test]
    fn new_guard_and_glob_require_a_reviewed_probe() -> Result<()> {
        ensure!(guard_signature("bittensor_core::keys::unreviewed").is_err());
        ensure!(inline_exports("pub use bittensor_core::keys::*;").is_err());
        Ok(())
    }

    #[test]
    fn external_and_path_modules_are_discovered_or_fail_closed() -> Result<()> {
        let temporary = std::env::var_os("RTAO_TEMP_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target"));
        let fixture = temporary.join(format!("surface-module-test-{}", std::process::id()));
        fs::create_dir_all(fixture.join("nested"))?;
        let result = (|| -> Result<()> {
            fs::write(
                fixture.join("lib.rs"),
                "#[cfg(feature = \"host\")] pub mod keys; #[path = \"renamed.rs\"] pub mod alternate; pub mod nested { pub mod child; }",
            )?;
            fs::write(
                fixture.join("keys.rs"),
                "pub use bittensor_core::keys::Keypair;",
            )?;
            fs::write(
                fixture.join("renamed.rs"),
                "pub use bittensor_core::codec::value::Value;",
            )?;
            fs::write(
                fixture.join("nested/child.rs"),
                "pub use bittensor_core::runtime::Runtime;",
            )?;
            let root = fixture.canonicalize()?;
            let source = root.join("lib.rs");
            let syntax = syn::parse_file(&fs::read_to_string(&source)?)?;
            let mut paths = BTreeSet::new();
            exports(
                &syntax.items,
                &[],
                &mut paths,
                &ExportSource {
                    path: &source,
                    directory: root.clone(),
                    inline: false,
                    root: &root,
                },
                &mut BTreeSet::from([source.clone()]),
            )?;
            ensure!(paths.len() == 3);
            ensure!(
                paths
                    .iter()
                    .any(|(cfg, path)| !cfg.is_empty() && path.ends_with("Keypair"))
            );
            fs::remove_file(root.join("keys.rs"))?;
            ensure!(
                exports(
                    &syntax.items,
                    &[],
                    &mut BTreeSet::new(),
                    &ExportSource {
                        path: &source,
                        directory: root.clone(),
                        inline: false,
                        root: &root
                    },
                    &mut BTreeSet::from([source.clone()])
                )
                .is_err()
            );
            let conditional =
                syn::parse_file("#[cfg_attr(feature = \"host\", path = \"keys.rs\")] mod keys;")?;
            ensure!(
                exports(
                    &conditional.items,
                    &[],
                    &mut BTreeSet::new(),
                    &ExportSource {
                        path: &source,
                        directory: root.clone(),
                        inline: false,
                        root: &root
                    },
                    &mut BTreeSet::from([source.clone()])
                )
                .is_err()
            );
            Ok(())
        })();
        // Only this test owns the fixture; it creates no child processes,
        // services, symlinks or external configuration referring to it.
        fs::remove_dir_all(&fixture)?;
        result
    }
}
