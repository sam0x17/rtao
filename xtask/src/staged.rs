//! Prove staged code is still absent from the pinned portable upstream core.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;
use toml_edit::DocumentMut;

#[derive(Debug, PartialEq, Eq)]
enum Probe {
    Symbol(String),
    Vectors(String),
}

#[derive(Debug, PartialEq, Eq)]
struct Header {
    target: String,
    probe: Probe,
    features: Vec<String>,
}

struct StagedModule {
    file: PathBuf,
    name: String,
    header: Header,
}

fn rust_path(path: &str) -> bool {
    !path.is_empty()
        && path.split("::").all(|part| {
            let mut bytes = part.bytes();
            bytes
                .next()
                .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
                && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        })
}

fn header(source: &str) -> Result<Header> {
    let read = |name: &str| -> Result<&str> {
        let prefix = format!("//! {name}: ");
        let found: Vec<_> = source
            .lines()
            .filter_map(|line| line.strip_prefix(&prefix))
            .collect();
        ensure!(
            found.len() == 1,
            "staged module needs exactly one {name} header"
        );
        Ok(found[0].trim())
    };
    let target = read("UPSTREAM-TARGET")?;
    let (path, pr) = target
        .split_once(" (PR-U")
        .context("UPSTREAM-TARGET must name its path and PR-U number")?;
    ensure!(rust_path(path), "invalid staged target path");
    ensure!(
        pr.strip_suffix(')')
            .is_some_and(|id| !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit())),
        "invalid upstream PR ID"
    );
    let probe = read("UPSTREAM-PROBE")?;
    let probe = if let Some(path) = probe.strip_prefix("symbol ") {
        ensure!(rust_path(path), "invalid symbol probe path");
        Probe::Symbol(path.to_owned())
    } else if let Some(suite) = probe.strip_prefix("vectors ") {
        ensure!(
            !suite.is_empty()
                && suite
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_'),
            "invalid vector suite name"
        );
        Probe::Vectors(suite.to_owned())
    } else {
        bail!("UPSTREAM-PROBE must specify symbol or vectors");
    };
    let features = read("UPSTREAM-FEATURES")?;
    let features: Vec<_> = if features == "none" {
        Vec::new()
    } else {
        features
            .split([',', ' '])
            .filter(|part| !part.is_empty())
            .map(str::to_owned)
            .collect()
    };
    ensure!(
        features.iter().all(|name| !name.is_empty()
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))),
        "invalid core feature name"
    );
    ensure!(
        features
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            == features.len(),
        "duplicate upstream feature"
    );
    Ok(Header {
        target: target.to_owned(),
        probe,
        features,
    })
}

fn files(root: &Path, result: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        ensure!(
            !entry.file_type()?.is_symlink(),
            "staged source is a symlink"
        );
        if entry.file_type()?.is_dir() {
            files(&entry.path(), result)?;
        } else if entry
            .path()
            .extension()
            .is_some_and(|extension| extension == "rs")
        {
            result.push(entry.path());
        }
    }
    Ok(())
}

fn header_attributes(attributes: &[syn::Attribute]) -> Result<Option<Header>> {
    let mut lines = Vec::new();
    for attribute in attributes {
        if !matches!(attribute.style, syn::AttrStyle::Inner(_)) || !attribute.path().is_ident("doc")
        {
            continue;
        }
        if let syn::Meta::NameValue(value) = &attribute.meta
            && let syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(text),
                ..
            }) = &value.value
            && text.value().trim_start().starts_with("UPSTREAM-")
        {
            lines.push(format!("//! {}", text.value().trim()));
        }
    }
    if lines.is_empty() {
        Ok(None)
    } else {
        header(&lines.join("\n")).map(Some)
    }
}

struct Discovery<'a> {
    root: &'a Path,
    active: BTreeSet<PathBuf>,
    visited: BTreeSet<PathBuf>,
    modules: Vec<StagedModule>,
}

struct ModuleSource<'a> {
    file: &'a Path,
    directory: PathBuf,
    inline: bool,
    name: String,
}

fn discover_items(
    items: &[syn::Item],
    attributes: &[syn::Attribute],
    source: &ModuleSource<'_>,
    guarded: bool,
    discovery: &mut Discovery<'_>,
) -> Result<()> {
    let own_header = header_attributes(attributes)?;
    let guarded = guarded || own_header.is_some();
    if let Some(header) = own_header {
        discovery.modules.push(StagedModule {
            file: source.file.to_owned(),
            name: source.name.clone(),
            header,
        });
    }
    for item in items {
        if let syn::Item::Mod(module) = item {
            ensure!(
                !module
                    .attrs
                    .iter()
                    .any(|attr| attr.path().is_ident("cfg_attr")),
                "conditional module attributes cannot bypass staged discovery: {}",
                module.ident
            );
            let name = if source.name == "lib" {
                module.ident.to_string()
            } else {
                format!("{}::{}", source.name, module.ident)
            };
            if let Some((_, items)) = &module.content {
                discover_items(
                    items,
                    &module.attrs,
                    &ModuleSource {
                        file: source.file,
                        directory: source.directory.join(module.ident.to_string()),
                        inline: true,
                        name,
                    },
                    guarded,
                    discovery,
                )?;
            } else {
                let base = if !source.inline
                    && module.attrs.iter().any(|attr| attr.path().is_ident("path"))
                {
                    source
                        .file
                        .parent()
                        .context("module source has no parent")?
                } else {
                    &source.directory
                };
                let child = crate::core_surface::module_file(module, base, discovery.root)?;
                ensure!(
                    discovery.active.insert(child.clone()),
                    "cyclic staged module source: {}",
                    child.display()
                );
                discovery.visited.insert(child.clone());
                let syntax = syn::parse_file(&fs::read_to_string(&child)?)?;
                discover_items(
                    &syntax.items,
                    &syntax.attrs,
                    &ModuleSource {
                        file: &child,
                        directory: crate::core_surface::module_directory(&child)?,
                        inline: false,
                        name,
                    },
                    guarded,
                    discovery,
                )?;
                discovery.active.remove(&child);
            }
        } else if !guarded {
            ensure!(
                matches!(item, syn::Item::Use(_) | syn::Item::ExternCrate(_)),
                "unguarded staged implementation in {} (module {}); add all three UPSTREAM headers",
                source.file.display(),
                source.name
            );
        }
    }
    Ok(())
}

fn staged_modules(root: &Path) -> Result<Vec<StagedModule>> {
    let root = root.canonicalize()?;
    let entry = root.join("lib.rs");
    let syntax = syn::parse_file(&fs::read_to_string(&entry)?)?;
    let mut discovery = Discovery {
        root: &root,
        active: BTreeSet::from([entry.clone()]),
        visited: BTreeSet::from([entry.clone()]),
        modules: Vec::new(),
    };
    discover_items(
        &syntax.items,
        &syntax.attrs,
        &ModuleSource {
            file: &entry,
            directory: root.clone(),
            inline: false,
            name: "lib".to_owned(),
        },
        false,
        &mut discovery,
    )?;
    let mut inventory = Vec::new();
    files(&root, &mut inventory)?;
    for file in inventory {
        ensure!(
            discovery.visited.contains(&file),
            "staged source is not reachable from lib.rs: {}",
            file.display()
        );
    }
    ensure!(
        !discovery.modules.is_empty(),
        "no staged modules were discovered"
    );
    discovery
        .modules
        .sort_by(|left, right| left.name.cmp(&right.name));
    Ok(discovery.modules)
}

struct ProbeIdentity {
    directory: PathBuf,
    manifest: PathBuf,
    source: PathBuf,
}

impl ProbeIdentity {
    fn new(directory: &Path) -> Result<Self> {
        let directory = directory.canonicalize()?;
        Ok(Self {
            manifest: directory.join("Cargo.toml"),
            source: directory.join("src/main.rs"),
            directory,
        })
    }

    fn matches(&self, record: &Value) -> bool {
        let package = record["package_id"]
            .as_str()
            .and_then(|id| id.split_once('#'));
        let package_matches = package.is_some_and(|(url, package)| {
            let Some(path) = url.strip_prefix("path+file://") else {
                return false;
            };
            let mut decoded = Vec::new();
            let mut bytes = path.bytes();
            while let Some(byte) = bytes.next() {
                if byte == b'%' {
                    let Some(high) = bytes.next().and_then(|byte| char::from(byte).to_digit(16))
                    else {
                        return false;
                    };
                    let Some(low) = bytes.next().and_then(|byte| char::from(byte).to_digit(16))
                    else {
                        return false;
                    };
                    decoded.push(u8::try_from(high * 16 + low).unwrap_or_default());
                } else {
                    decoded.push(byte);
                }
            }
            let Ok(path) = String::from_utf8(decoded) else {
                return false;
            };
            Path::new(&path) == self.directory
                && (package == "rtao-staged-probe@0.0.0"
                    || (package == "0.0.0"
                        && self
                            .directory
                            .file_name()
                            .is_some_and(|name| name == "rtao-staged-probe")))
        });
        package_matches
            && record["manifest_path"]
                .as_str()
                .is_some_and(|path| Path::new(path) == self.manifest)
            && record["target"]["name"].as_str() == Some("rtao-staged-probe")
            && record["target"]["kind"] == serde_json::json!(["bin"])
            && record["target"]["src_path"]
                .as_str()
                .is_some_and(|path| Path::new(path) == self.source)
    }
}

fn unresolved_in_probe(output: &[u8], identity: &ProbeIdentity) -> Result<bool> {
    let mut errors = Vec::new();
    for line in std::str::from_utf8(output)?.lines() {
        let Ok(record) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if record["reason"].as_str() == Some("compiler-message")
            && record["message"]["level"].as_str() == Some("error")
        {
            if !identity.matches(&record) {
                return Ok(false);
            }
            errors.push(record["message"].clone());
        }
    }
    if errors.is_empty() {
        return Ok(false);
    }
    for error in errors {
        let code = error["code"]["code"].as_str().unwrap_or("");
        if !["E0405", "E0412", "E0425", "E0432", "E0433", "E0599"].contains(&code) {
            return Ok(false);
        }
        let own_source = error["spans"].as_array().is_some_and(|spans| {
            spans.iter().any(|span| {
                if span["is_primary"].as_bool() != Some(true) {
                    return false;
                }
                span["file_name"].as_str().is_some_and(|name| {
                    let name = Path::new(name);
                    name == identity.source
                        || (!name.is_absolute() && identity.directory.join(name) == identity.source)
                })
            })
        });
        if !own_source {
            return Ok(false);
        }
    }
    Ok(true)
}

fn source(probe: &Probe, root: &Path) -> Result<String> {
    match probe {
        Probe::Symbol(path) => {
            // Importing a free symbol proves its existence without needing to
            // infer generic type arguments. Associated methods cannot be
            // imported; their conventional type-qualified paths need an
            // expression (generic associated methods use a typed vector suite).
            let associated = path
                .rsplit("::")
                .nth(1)
                .and_then(|part| part.bytes().next())
                .is_some_and(|first| first.is_ascii_uppercase());
            Ok(if !associated {
                format!("#[allow(unused_imports)]\nuse bittensor_core::{path};\nfn main() {{}}\n")
            } else {
                format!("fn main() {{ let _ = bittensor_core::{path}; }}\n")
            })
        }
        Probe::Vectors(suite) => {
            // A suite is committed with the staged module and must directly name
            // the upstream implementation. It runs inside this isolated crate.
            let source =
                fs::read_to_string(root.join(format!("crates/rtao-core-ext/probes/{suite}.rs")))
                    .with_context(|| format!("missing staged vector suite {suite}"))?;
            let syntax = syn::parse_file(&source)?;
            ensure!(syntax.items.iter().any(|item| matches!(item, syn::Item::Fn(function) if function.attrs.iter().any(|attribute| attribute.path().is_ident("test")))), "vector suite has no top-level test; refusing a vacuous probe");
            struct CoreReference(bool);
            impl<'ast> syn::visit::Visit<'ast> for CoreReference {
                fn visit_path(&mut self, path: &'ast syn::Path) {
                    self.0 |= path
                        .segments
                        .first()
                        .is_some_and(|segment| segment.ident == "bittensor_core");
                    syn::visit::visit_path(self, path);
                }

                fn visit_use_tree(&mut self, tree: &'ast syn::UseTree) {
                    self.0 |=
                        matches!(tree, syn::UseTree::Path(path) if path.ident == "bittensor_core");
                    syn::visit::visit_use_tree(self, tree);
                }

                fn visit_macro(&mut self, node: &'ast syn::Macro) {
                    // syn leaves assertion arguments as tokens. Parse comma-
                    // separated expressions so paths inside assert!/assert_eq!
                    // count, while comments and string literals still do not.
                    if let Ok(arguments) = node.parse_body_with(
                        syn::punctuated::Punctuated::<syn::Expr, syn::Token![,]>::parse_terminated,
                    ) {
                        for argument in arguments {
                            let mut nested = CoreReference(false);
                            syn::visit::Visit::visit_expr(&mut nested, &argument);
                            self.0 |= nested.0;
                        }
                    }
                    syn::visit::visit_macro(self, node);
                }
            }
            let mut reference = CoreReference(false);
            syn::visit::Visit::visit_file(&mut reference, &syntax);
            ensure!(reference.0, "vector suite must call the upstream core");
            Ok(format!("{source}\nfn main() {{}}\n"))
        }
    }
}

fn compile(
    directory: &Path,
    probe: &Probe,
    repo: &str,
    tag: &str,
    features: &[String],
) -> Result<std::process::Output> {
    let manifest = format!(
        "[workspace]\n[package]\nname = \"rtao-staged-probe\"\nversion = \"0.0.0\"\nedition = \"2024\"\n[dependencies]\nbittensor-core = {{ git = {repo:?}, tag = {tag:?}, default-features = false, features = {} }}\n[patch.crates-io]\nw3f-bls = {{ git = {repo:?}, tag = {tag:?} }}\n",
        serde_json::to_string(features)?
    );
    fs::write(directory.join("Cargo.toml"), manifest)?;
    Command::new("cargo")
        .current_dir(directory)
        // Manifests and locks stay isolated; only compiled dependency artifacts
        // are shared between the module probes and their host comparisons.
        .env(
            "CARGO_TARGET_DIR",
            directory
                .parent()
                .context("probe directory has no parent")?
                .join(".build-cache"),
        )
        .args([
            if matches!(probe, Probe::Vectors(_)) {
                "test"
            } else {
                "check"
            },
            "--message-format=json",
        ])
        .output()
        .context("launch staged probe")
}

#[derive(Debug, PartialEq, Eq)]
enum ProbeOutcome {
    Present,
    MissingSymbol,
    FailedVectors,
}

fn vector_summary(output: &str) -> Result<Option<(u64, u64)>> {
    let mut summary = None;
    for line in output
        .lines()
        .filter(|line| line.starts_with("test result: "))
    {
        let (_, counts) = line
            .split_once(". ")
            .context("malformed vector test summary")?;
        let count = |name: &str| -> Result<u64> {
            let text = counts
                .split(';')
                .map(str::trim)
                .find_map(|field| field.strip_suffix(name))
                .with_context(|| format!("vector summary missing {name}"))?;
            Ok(text.trim().parse()?)
        };
        let passed = count(" passed")?;
        let failed = count(" failed")?;
        ensure!(passed + failed > 0, "vector suite executed no tests");
        ensure!(
            count(" ignored")? == 0 && count(" filtered out")? == 0,
            "vector suite skipped tests"
        );
        ensure!(
            summary.is_none(),
            "vector probe produced more than one test summary"
        );
        summary = Some((passed, failed));
    }
    Ok(summary)
}

fn classify(
    output: &std::process::Output,
    probe: &Probe,
    identity: &ProbeIdentity,
) -> Result<ProbeOutcome> {
    if unresolved_in_probe(&output.stdout, identity)? {
        ensure!(
            !output.status.success(),
            "successful probe emitted unresolved errors"
        );
        return Ok(ProbeOutcome::MissingSymbol);
    }
    if matches!(probe, Probe::Symbol(_)) && output.status.success() {
        return Ok(ProbeOutcome::Present);
    }
    if matches!(probe, Probe::Vectors(_)) {
        let mut built_tests = false;
        let mut finished = false;
        let mut compiler_error = false;
        for line in std::str::from_utf8(&output.stdout)?.lines() {
            let Ok(record) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            match record["reason"].as_str() {
                Some("compiler-artifact") if identity.matches(&record) => {
                    built_tests |= record["profile"]["test"].as_bool() == Some(true)
                        && record["executable"].as_str().is_some();
                }
                Some("build-finished") => finished = record["success"].as_bool() == Some(true),
                Some("compiler-message") => {
                    compiler_error |= record["message"]["level"].as_str() == Some("error")
                }
                _ => {}
            }
        }
        if built_tests
            && finished
            && !compiler_error
            && let Some((_, failed)) = vector_summary(std::str::from_utf8(&output.stdout)?)?
        {
            ensure!(
                output.status.success() == (failed == 0),
                "vector test summary disagrees with Cargo status"
            );
            return Ok(if failed == 0 {
                ProbeOutcome::Present
            } else {
                ProbeOutcome::FailedVectors
            });
        }
    }
    bail!(
        "probe failed for an unexpected reason:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

struct FeatureProbes<'a> {
    missing: Option<&'a str>,
    host: Option<Vec<String>>,
}

fn feature_probes<'a>(
    features: &'a [String],
    upstream: &dyn toml_edit::TableLike,
) -> FeatureProbes<'a> {
    let missing = features
        .iter()
        .find(|feature| !upstream.contains_key(feature.as_str()))
        .map(String::as_str);
    let host = if !features.iter().any(|feature| feature == "host") && upstream.contains_key("host")
    {
        let mut host: Vec<_> = features
            .iter()
            .filter(|feature| upstream.contains_key(feature.as_str()))
            .cloned()
            .collect();
        host.push("host".to_owned());
        Some(host)
    } else {
        None
    };
    FeatureProbes { missing, host }
}

pub(crate) fn run(args: Vec<String>) -> Result<()> {
    ensure!(args == ["--check"], "usage: cargo xtask staged --check");
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .context("workspace parent missing")?;
    let pins = crate::pin::read_document(&root.join("upstream.toml"))?;
    let repo = pins["monorepo"]["url"]
        .as_str()
        .context("missing monorepo.url")?;
    let tag = pins["core"]["tag"].as_str().context("missing core.tag")?;
    let commit = pins["core"]["commit"]
        .as_str()
        .context("missing core.commit")?;
    let cache = crate::pin::cached_repo(root)?;
    let upstream = crate::pin::git(
        &cache,
        &["show", &format!("{commit}:sdk/bittensor-core/Cargo.toml")],
    )?;
    let upstream: DocumentMut = std::str::from_utf8(&upstream)?.parse()?;
    let feature_table = upstream["features"]
        .as_table_like()
        .context("core feature table missing")?;
    let temporary = std::env::var_os("RTAO_TEMP_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("target"));
    let probes = temporary.join("staged-probe");
    let modules = staged_modules(&root.join("crates/rtao-core-ext/src"))?;
    for module in &modules {
        let header = &module.header;
        let name = module.name.replace("::", "_");
        let directory = probes.join(name);
        fs::create_dir_all(directory.join("src"))?;
        fs::copy(root.join("Cargo.lock"), directory.join("Cargo.lock"))?;
        let probe_source = directory.join("src/main.rs");
        fs::write(&probe_source, source(&header.probe, root)?)?;
        let features = feature_probes(&header.features, feature_table);
        let identity = ProbeIdentity::new(&directory)?;
        if let Some(feature) = features.missing {
            println!(
                "{}: not upstream yet (missing core feature {feature})",
                header.target
            );
        } else {
            let output = compile(&directory, &header.probe, repo, tag, &header.features)?;
            fs::write(directory.join("compiler.jsonl"), &output.stdout)?;
            fs::write(directory.join("compiler.stderr"), &output.stderr)?;
            let outcome = classify(&output, &header.probe, &identity)
                .with_context(|| format!("{} portable probe", header.target))?;
            ensure!(
                outcome != ProbeOutcome::Present,
                "{} is upstream: remove staged module {} ({})",
                header.target,
                module.name,
                module.file.display()
            );
        }
        if let Some(host) = features.host {
            // When a new portable feature is absent, the existing host feature
            // can still expose the old implementation. This comparison never
            // retires code; the portable probe above always uses exact features.
            let hosted = compile(&directory, &header.probe, repo, tag, &host)?;
            fs::write(directory.join("host-compiler.jsonl"), &hosted.stdout)?;
            fs::write(directory.join("host-compiler.stderr"), &hosted.stderr)?;
            if classify(&hosted, &header.probe, &identity)
                .with_context(|| format!("{} host comparison", header.target))?
                == ProbeOutcome::Present
            {
                println!(
                    "{}: upstream but host-gated; staged portable module retained",
                    header.target
                );
                continue;
            }
        }
        println!("{}: not upstream yet", header.target);
    }
    println!("checked {} staged modules", modules.len());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture(name: &str, test: impl FnOnce(&Path) -> Result<()>) -> Result<()> {
        let temporary = std::env::var_os("RTAO_TEMP_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target"));
        let directory = temporary.join(format!("staged-{name}-test-{}", std::process::id()));
        fs::create_dir_all(&directory)?;
        let result = test(&directory);
        // Cargo output() calls have reaped every child before returning. Tests
        // create no services, symlinks, venvs or external configuration paths.
        fs::remove_dir_all(&directory)?;
        result
    }

    fn mock_core(directory: &Path, source: &str) -> Result<()> {
        fs::create_dir_all(directory.join("core/src"))?;
        fs::create_dir_all(directory.join("probe/src"))?;
        fs::write(
            directory.join("core/Cargo.toml"),
            "[package]\nname = \"bittensor-core\"\nversion = \"0.0.0\"\nedition = \"2024\"\n[features]\ndefault = []\nhost = []\n",
        )?;
        fs::write(directory.join("core/src/lib.rs"), source)?;
        Ok(())
    }

    fn local_probe(
        directory: &Path,
        probe: &Probe,
        features: &[&str],
        flags: Option<&str>,
    ) -> Result<std::process::Output> {
        let probe_directory = directory.join("probe");
        fs::write(
            probe_directory.join("Cargo.toml"),
            format!(
                "[workspace]\n[package]\nname = \"rtao-staged-probe\"\nversion = \"0.0.0\"\nedition = \"2024\"\n[dependencies]\nbittensor-core = {{ path = \"../core\", default-features = false, features = {} }}\n",
                serde_json::to_string(features)?
            ),
        )?;
        fs::write(
            probe_directory.join("src/main.rs"),
            source(probe, directory)?,
        )?;
        let mut cargo = Command::new("cargo");
        cargo
            .current_dir(&probe_directory)
            .env("CARGO_TARGET_DIR", directory.join("target"))
            .args([
                if matches!(probe, Probe::Vectors(_)) {
                    "test"
                } else {
                    "check"
                },
                "--offline",
                "--message-format=json",
            ]);
        if let Some(flags) = flags {
            cargo.env("RUSTFLAGS", flags);
        }
        Ok(cargo.output()?)
    }

    #[test]
    fn headers_reject_duplicates_malformed_paths_and_features() -> Result<()> {
        let valid = "//! UPSTREAM-TARGET: keys::verify_strict (PR-U10)\n//! UPSTREAM-PROBE: symbol keys::verify_strict\n//! UPSTREAM-FEATURES: none\n";
        ensure!(header(valid)?.features.is_empty());
        ensure!(header(&format!("{valid}//! UPSTREAM-PROBE: symbol other\n")).is_err());
        ensure!(header(&valid.replace("keys::verify_strict", "../../bad")).is_err());
        ensure!(header(&valid.replace("FEATURES: none", "FEATURES: host,host")).is_err());
        Ok(())
    }

    #[test]
    fn only_own_unresolved_name_errors_count_as_absence() -> Result<()> {
        let record = |code: &str, file: &str| {
            json!({"reason":"compiler-message", "package_id":"path+file:///task/probe#rtao-staged-probe@0.0.0", "manifest_path":"/task/probe/Cargo.toml", "target":{"name":"rtao-staged-probe", "kind":["bin"], "src_path":"/task/probe/src/main.rs"}, "message":{"level":"error","code":{"code":code},"spans":[{"is_primary":true,"file_name":file}]}}).to_string()
        };
        let identity = ProbeIdentity {
            directory: PathBuf::from("/task/probe"),
            manifest: PathBuf::from("/task/probe/Cargo.toml"),
            source: PathBuf::from("/task/probe/src/main.rs"),
        };
        ensure!(unresolved_in_probe(
            record("E0425", "src/main.rs").as_bytes(),
            &identity
        )?);
        ensure!(!unresolved_in_probe(
            record("E0277", "src/main.rs").as_bytes(),
            &identity
        )?);
        ensure!(!unresolved_in_probe(
            record("E0425", "/cargo/dep/src/main.rs").as_bytes(),
            &identity
        )?);
        ensure!(!unresolved_in_probe(b"download failed", &identity)?);
        let combined = format!(
            "{}\n{}",
            record("E0425", "src/main.rs"),
            record("E0277", "src/main.rs")
        );
        ensure!(!unresolved_in_probe(combined.as_bytes(), &identity)?);
        let mut wrong_package: Value = serde_json::from_str(&record("E0433", "src/main.rs"))?;
        wrong_package["package_id"] = json!("path+file:///task/dep#dep@0.0.0");
        ensure!(!unresolved_in_probe(
            wrong_package.to_string().as_bytes(),
            &identity
        )?);
        let mut wrong_target: Value = serde_json::from_str(&record("E0433", "src/main.rs"))?;
        wrong_target["target"]["kind"] = json!(["custom-build"]);
        ensure!(!unresolved_in_probe(
            wrong_target.to_string().as_bytes(),
            &identity
        )?);
        Ok(())
    }

    #[test]
    fn inline_headers_external_paths_and_unguarded_code_are_checked() -> Result<()> {
        fixture("discovery", |directory| {
            let headers = "//! UPSTREAM-TARGET: keys::verify_strict (PR-U10)\n//! UPSTREAM-PROBE: symbol keys::verify_strict\n//! UPSTREAM-FEATURES: none\n";
            fs::write(
                directory.join("lib.rs"),
                format!(
                    "#[path = \"renamed.rs\"] pub mod external; pub mod inline {{\n{headers} pub fn inline_code() {{}}\n}}"
                ),
            )?;
            fs::write(
                directory.join("renamed.rs"),
                format!("{headers}pub fn external_code() {{}}"),
            )?;
            let modules = staged_modules(directory)?;
            ensure!(
                modules.len() == 2
                    && modules.iter().any(|module| module.name == "inline")
                    && modules.iter().any(|module| module.name == "external")
            );
            fs::write(
                directory.join("lib.rs"),
                "#[path = \"renamed.rs\"] pub mod external; pub mod inline { pub fn unguarded() {} }",
            )?;
            ensure!(staged_modules(directory).is_err());
            fs::write(
                directory.join("lib.rs"),
                "#[path = \"renamed.rs\"] pub mod external;",
            )?;
            fs::write(
                directory.join("forgotten.rs"),
                format!("{headers}pub fn forgotten() {{}}"),
            )?;
            ensure!(staged_modules(directory).is_err());
            Ok(())
        })
    }

    #[test]
    fn generic_free_symbols_and_associated_methods_compile() -> Result<()> {
        fixture("generic", |directory| {
            mock_core(
                directory,
                "pub mod keys { pub fn generic<T: AsRef<[u8]>>(_value: T) {} } pub mod runtime { pub struct Runtime; impl Runtime { pub fn metadata_ir(&self) {} } }",
            )?;
            for path in ["keys::generic", "runtime::Runtime::metadata_ir"] {
                let probe = Probe::Symbol(path.to_owned());
                let output = local_probe(directory, &probe, &[], None)?;
                ensure!(
                    classify(
                        &output,
                        &probe,
                        &ProbeIdentity::new(&directory.join("probe"))?
                    )? == ProbeOutcome::Present
                );
            }
            Ok(())
        })
    }

    #[test]
    fn portable_and_host_probes_share_symbol_and_vector_classification() -> Result<()> {
        fixture("host-vector", |directory| {
            mock_core(
                directory,
                "pub mod keys { #[cfg(feature = \"host\")] pub fn hosted() -> bool { false } }",
            )?;
            let probe = Probe::Symbol("keys::hosted".to_owned());
            let identity = ProbeIdentity::new(&directory.join("probe"))?;
            ensure!(
                classify(
                    &local_probe(directory, &probe, &[], None)?,
                    &probe,
                    &identity
                )? == ProbeOutcome::MissingSymbol
            );
            ensure!(
                classify(
                    &local_probe(directory, &probe, &["host"], None)?,
                    &probe,
                    &identity
                )? == ProbeOutcome::Present
            );
            let suites = directory.join("crates/rtao-core-ext/probes");
            fs::create_dir_all(&suites)?;
            fs::write(
                suites.join("hosted.rs"),
                "#[test] fn vector() { assert!(bittensor_core::keys::hosted()); }",
            )?;
            let probe = Probe::Vectors("hosted".to_owned());
            ensure!(
                classify(
                    &local_probe(directory, &probe, &[], None)?,
                    &probe,
                    &identity
                )? == ProbeOutcome::MissingSymbol
            );
            ensure!(
                classify(
                    &local_probe(directory, &probe, &["host"], None)?,
                    &probe,
                    &identity
                )? == ProbeOutcome::FailedVectors
            );
            fs::write(
                directory.join("core/src/lib.rs"),
                "pub mod keys { #[cfg(feature = \"host\")] pub fn hosted() -> bool { true } }",
            )?;
            ensure!(
                classify(
                    &local_probe(directory, &probe, &["host"], None)?,
                    &probe,
                    &identity
                )? == ProbeOutcome::Present
            );
            fs::write(
                suites.join("hosted.rs"),
                "#[test] #[ignore] fn vector() { assert!(bittensor_core::keys::hosted()); }",
            )?;
            ensure!(
                classify(
                    &local_probe(directory, &probe, &["host"], None)?,
                    &probe,
                    &identity
                )
                .is_err()
            );
            fs::write(
                suites.join("hosted.rs"),
                "// bittensor_core::keys::hosted()\n#[test] fn unrelated() { assert!(true); }",
            )?;
            ensure!(source(&probe, directory).is_err());
            Ok(())
        })
    }

    #[test]
    fn remapped_dependency_errors_never_count_as_upstream_absence() -> Result<()> {
        fixture("remapped-dependency", |directory| {
            mock_core(directory, "pub fn unused() {}")?;
            fs::write(
                directory.join("core/Cargo.toml"),
                "[package]\nname = \"bittensor-core\"\nversion = \"0.0.0\"\nedition = \"2024\"\nbuild = \"src/main.rs\"\nautobins = false\n",
            )?;
            fs::write(
                directory.join("core/src/main.rs"),
                "fn main() { broken_dependency_module::run(); }",
            )?;
            let flags = format!(
                "--remap-path-prefix={}/=",
                directory.join("core").canonicalize()?.display()
            );
            let probe = Probe::Symbol("missing".to_owned());
            let output = local_probe(directory, &probe, &[], Some(&flags))?;
            let identity = ProbeIdentity::new(&directory.join("probe"))?;
            let remapped = std::str::from_utf8(&output.stdout)?
                .lines()
                .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                .any(|record| {
                    record["reason"] == "compiler-message"
                        && record["message"]["level"] == "error"
                        && record["message"]["spans"].as_array().is_some_and(|spans| {
                            spans.iter().any(|span| span["file_name"] == "src/main.rs")
                        })
                });
            ensure!(
                remapped,
                "regression must exercise an actual remapped dependency diagnostic"
            );
            ensure!(!unresolved_in_probe(&output.stdout, &identity)?);
            ensure!(classify(&output, &probe, &identity).is_err());
            Ok(())
        })
    }

    #[test]
    fn missing_portable_feature_still_proves_the_existing_host_export() -> Result<()> {
        fixture("missing-feature", |directory| {
            mock_core(
                directory,
                "pub mod keys { #[cfg(feature = \"host\")] pub fn hosted() {} }",
            )?;
            let manifest: DocumentMut =
                fs::read_to_string(directory.join("core/Cargo.toml"))?.parse()?;
            let requested = vec!["keyfiles-json".to_owned()];
            let features = feature_probes(
                &requested,
                manifest["features"]
                    .as_table_like()
                    .context("mock feature table")?,
            );
            ensure!(features.missing == Some("keyfiles-json"));
            let host = features
                .host
                .context("missing portable feature must not skip the host comparison")?;
            let host: Vec<_> = host.iter().map(String::as_str).collect();
            let probe = Probe::Symbol("keys::hosted".to_owned());
            let identity = ProbeIdentity::new(&directory.join("probe"))?;
            ensure!(
                classify(
                    &local_probe(directory, &probe, &host, None)?,
                    &probe,
                    &identity
                )? == ProbeOutcome::Present
            );
            // Cargo confirms that requesting the missing feature is resolution
            // failure, so absence is accepted only from the pinned manifest.
            ensure!(
                classify(
                    &local_probe(directory, &probe, &["keyfiles-json"], None)?,
                    &probe,
                    &identity
                )
                .is_err()
            );
            Ok(())
        })
    }
}
