//! Structural Python symbol reports and the Rust alias/test indexes.

use anyhow::{Context, Result, bail, ensure};
use quote::ToTokens;
use rustpython_parser::{
    Mode, Parse,
    ast::{self, Ranged},
    lexer,
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    process::Command,
};
use syn::{
    parse::Parser,
    visit::{self, Visit},
};

#[derive(Clone, Debug)]
pub(super) struct Location {
    pub path: String,
    pub line: usize,
    pub item: String,
}

#[derive(Default)]
pub(super) struct Index {
    pub aliases: BTreeMap<String, Vec<Location>>,
    pub tests: BTreeSet<String>,
    pub test_bodies: BTreeMap<String, String>,
    pub rejected_tests: BTreeMap<String, String>,
    pub feature_tests: BTreeSet<String>,
    pub feature_test_bodies: BTreeMap<String, String>,
    pub rejected_feature_tests: BTreeMap<String, String>,
}

fn aliases(attrs: &[syn::Attribute]) -> Result<Vec<String>> {
    let mut values = Vec::new();
    for attr in attrs.iter().filter(|attr| attr.path().is_ident("doc")) {
        let syn::Meta::List(list) = &attr.meta else {
            continue;
        };
        let entries = syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated
            .parse2(list.tokens.clone())?;
        for entry in entries {
            match entry {
                syn::Meta::NameValue(entry) if entry.path.is_ident("alias") => {
                    let syn::Expr::Lit(syn::ExprLit {
                        lit: syn::Lit::Str(value),
                        ..
                    }) = entry.value
                    else {
                        bail!("doc alias must be a string")
                    };
                    values.push(value.value());
                }
                syn::Meta::List(entry) if entry.path.is_ident("alias") => {
                    values.extend(syn::punctuated::Punctuated::<syn::LitStr,syn::Token![,]>::parse_terminated.parse2(entry.tokens)?.into_iter().map(|value|value.value()));
                }
                _ => {}
            }
        }
    }
    Ok(values)
}

struct RustScanner<'a> {
    index: &'a mut Index,
    path: String,
    source: &'a str,
    scope: Vec<String>,
    error: Option<anyhow::Error>,
}
impl RustScanner<'_> {
    fn record(&mut self, name: &str, attrs: &[syn::Attribute]) {
        let item = self
            .scope
            .iter()
            .map(String::as_str)
            .chain([name])
            .collect::<Vec<_>>()
            .join("::");
        match aliases(attrs) {
            Ok(aliases) => {
                for alias in aliases
                    .into_iter()
                    .filter(|alias| alias.starts_with("bittensor."))
                {
                    let needle = format!("{alias:?}");
                    let line = self.source.find(&needle).map_or(1, |offset| {
                        self.source[..offset]
                            .bytes()
                            .filter(|byte| *byte == b'\n')
                            .count()
                            + 1
                    });
                    self.index.aliases.entry(alias).or_default().push(Location {
                        path: self.path.clone(),
                        line,
                        item: item.clone(),
                    });
                }
            }
            Err(error) => self.error = Some(error),
        }
    }
}
impl<'ast> Visit<'ast> for RustScanner<'_> {
    fn visit_item_mod(&mut self, item: &'ast syn::ItemMod) {
        self.record(&item.ident.to_string(), &item.attrs);
        self.scope.push(item.ident.to_string());
        visit::visit_item_mod(self, item);
        self.scope.pop();
    }
    fn visit_item_fn(&mut self, item: &'ast syn::ItemFn) {
        self.record(&item.sig.ident.to_string(), &item.attrs);
        let name = self
            .scope
            .iter()
            .map(String::as_str)
            .chain([item.sig.ident.to_string().as_str()])
            .collect::<Vec<_>>()
            .join("::");
        if test_attributes(&item.attrs, &FeatureContext::default()).0 {
            self.index
                .rejected_tests
                .entry(name)
                .or_insert_with(|| "not reachable from a Cargo test target".into());
        }
    }
    fn visit_impl_item_fn(&mut self, item: &'ast syn::ImplItemFn) {
        self.record(&item.sig.ident.to_string(), &item.attrs);
        visit::visit_impl_item_fn(self, item);
    }
    fn visit_item_impl(&mut self, item: &'ast syn::ItemImpl) {
        let name = item.self_ty.to_token_stream().to_string().replace(' ', "");
        self.scope.push(name);
        visit::visit_item_impl(self, item);
        self.scope.pop();
    }
    fn visit_item_struct(&mut self, item: &'ast syn::ItemStruct) {
        self.record(&item.ident.to_string(), &item.attrs);
        visit::visit_item_struct(self, item);
    }
    fn visit_item_enum(&mut self, item: &'ast syn::ItemEnum) {
        self.record(&item.ident.to_string(), &item.attrs);
        visit::visit_item_enum(self, item);
    }
    fn visit_item_const(&mut self, item: &'ast syn::ItemConst) {
        self.record(&item.ident.to_string(), &item.attrs);
        visit::visit_item_const(self, item);
    }
    fn visit_item_static(&mut self, item: &'ast syn::ItemStatic) {
        self.record(&item.ident.to_string(), &item.attrs);
        visit::visit_item_static(self, item);
    }
    fn visit_item_type(&mut self, item: &'ast syn::ItemType) {
        self.record(&item.ident.to_string(), &item.attrs);
        visit::visit_item_type(self, item);
    }
    fn visit_item_trait(&mut self, item: &'ast syn::ItemTrait) {
        self.record(&item.ident.to_string(), &item.attrs);
        visit::visit_item_trait(self, item);
    }
    fn visit_item_use(&mut self, item: &'ast syn::ItemUse) {
        self.record(&item.tree.to_token_stream().to_string(), &item.attrs);
        visit::visit_item_use(self, item);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Condition {
    Always,
    Never,
    Unknown,
}

#[derive(Clone, Default)]
struct FeatureContext {
    declared: BTreeSet<String>,
    enabled: BTreeSet<String>,
}

fn meta_list(list: &syn::MetaList) -> Result<Vec<syn::Meta>> {
    Ok(
        syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated
            .parse2(list.tokens.clone())?
            .into_iter()
            .collect(),
    )
}

// Target platform selectors remain unknown in every source-index context.
fn test_condition(meta: &syn::Meta, features: &FeatureContext) -> Result<Condition> {
    match meta {
        syn::Meta::Path(path) if path.is_ident("test") => Ok(Condition::Always),
        syn::Meta::NameValue(syn::MetaNameValue {
            path,
            value:
                syn::Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Str(value),
                    ..
                }),
            ..
        }) if path.is_ident("feature") => {
            let name = value.value();
            Ok(if !features.declared.contains(&name) {
                Condition::Unknown
            } else if features.enabled.contains(&name) {
                Condition::Always
            } else {
                Condition::Never
            })
        }
        syn::Meta::List(list) if list.path.is_ident("all") || list.path.is_ident("any") => {
            let conditions = meta_list(list)?
                .iter()
                .map(|meta| test_condition(meta, features))
                .collect::<Result<Vec<_>>>()?;
            if list.path.is_ident("all") {
                Ok(if conditions.contains(&Condition::Never) {
                    Condition::Never
                } else if conditions.contains(&Condition::Unknown) {
                    Condition::Unknown
                } else {
                    Condition::Always
                })
            } else {
                Ok(if conditions.contains(&Condition::Always) {
                    Condition::Always
                } else if conditions.contains(&Condition::Unknown) {
                    Condition::Unknown
                } else {
                    Condition::Never
                })
            }
        }
        syn::Meta::List(list) if list.path.is_ident("not") => {
            let conditions = meta_list(list)?;
            ensure!(conditions.len() == 1, "cfg not requires one predicate");
            Ok(match test_condition(&conditions[0], features)? {
                Condition::Always => Condition::Never,
                Condition::Never => Condition::Always,
                Condition::Unknown => Condition::Unknown,
            })
        }
        _ => Ok(Condition::Unknown),
    }
}

fn test_attribute_meta(
    meta: &syn::Meta,
    features: &FeatureContext,
) -> Result<(bool, Option<String>)> {
    let path = meta.path();
    let rendered = meta.to_token_stream().to_string();
    if path.is_ident("ignore") {
        return Ok((false, Some(format!("ignored by #[{rendered}]"))));
    }
    if path.is_ident("cfg") {
        let syn::Meta::List(list) = meta else {
            bail!("cfg requires a predicate")
        };
        let conditions = meta_list(list)?;
        ensure!(conditions.len() == 1, "cfg requires one predicate");
        return Ok((
            false,
            (test_condition(&conditions[0], features)? != Condition::Always)
                .then(|| format!("conditionally omitted by #[{rendered}]")),
        ));
    }
    if path.is_ident("cfg_attr") {
        let syn::Meta::List(list) = meta else {
            bail!("cfg_attr requires a predicate and attributes")
        };
        let entries = meta_list(list)?;
        ensure!(entries.len() >= 2, "cfg_attr requires attributes");
        let condition = test_condition(&entries[0], features)?;
        let mut candidate = false;
        let mut rejection = None;
        for entry in entries.iter().skip(1) {
            let (is_test, reason) = test_attribute_meta(entry, features)?;
            if condition != Condition::Never {
                candidate |= is_test;
                if condition == Condition::Unknown
                    && (is_test || reason.is_some() || can_set_module_path(entry, features)?)
                {
                    rejection = Some(format!("conditional test attributes in #[{rendered}]"));
                } else if reason.is_some() {
                    rejection = reason;
                }
            }
        }
        return Ok((candidate, rejection));
    }
    if path.is_ident("test")
        || path
            .segments
            .iter()
            .map(|part| part.ident.to_string())
            .eq(["tokio", "test"].map(str::to_owned))
    {
        return Ok((true, None));
    }
    if path.segments.last().is_some_and(|part| {
        part.ident == "test" || part.ident == "rstest" || part.ident == "test_case"
    }) {
        return Ok((
            true,
            Some(format!("unsupported test attribute #[{rendered}]")),
        ));
    }
    Ok((false, None))
}

fn can_set_module_path(meta: &syn::Meta, features: &FeatureContext) -> Result<bool> {
    if meta.path().is_ident("path") {
        return Ok(true);
    }
    let syn::Meta::List(list) = meta else {
        return Ok(false);
    };
    if list.path.is_ident("cfg_attr") {
        let entries = meta_list(list)?;
        ensure!(entries.len() >= 2, "cfg_attr requires attributes");
        if test_condition(&entries[0], features)? != Condition::Never {
            for entry in entries.iter().skip(1) {
                if can_set_module_path(entry, features)? {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

fn test_attributes(attrs: &[syn::Attribute], features: &FeatureContext) -> (bool, Option<String>) {
    let mut candidate = false;
    let mut rejection = None;
    for attr in attrs {
        match test_attribute_meta(&attr.meta, features) {
            Ok((is_test, reason)) => {
                candidate |= is_test;
                if rejection.is_none() {
                    rejection = reason;
                }
            }
            Err(error) => rejection = Some(format!("invalid test attributes: {error}")),
        }
    }
    (candidate, rejection)
}

#[derive(Clone)]
struct TestSource {
    path: PathBuf,
    scope: Vec<String>,
    module_dir: PathBuf,
    path_dir: PathBuf,
    rejection: Option<String>,
    features: FeatureContext,
    ancestors: Vec<PathBuf>,
}

struct TestScanner<'a> {
    index: &'a mut Index,
    context: TestSource,
    queued: &'a mut Vec<TestSource>,
    error: Option<anyhow::Error>,
}

fn module_path(attrs: &[syn::Attribute], features: &FeatureContext) -> Result<Option<PathBuf>> {
    fn paths(meta: &syn::Meta, values: &mut Vec<PathBuf>, features: &FeatureContext) -> Result<()> {
        if meta.path().is_ident("cfg_attr") {
            let syn::Meta::List(list) = meta else {
                bail!("cfg_attr requires a predicate and attributes")
            };
            let entries = meta_list(list)?;
            ensure!(entries.len() >= 2, "cfg_attr requires attributes");
            if test_condition(&entries[0], features)? == Condition::Always {
                for entry in entries.iter().skip(1) {
                    paths(entry, values, features)?;
                }
            }
        } else if meta.path().is_ident("path") {
            let syn::Meta::NameValue(syn::MetaNameValue {
                value:
                    syn::Expr::Lit(syn::ExprLit {
                        lit: syn::Lit::Str(value),
                        ..
                    }),
                ..
            }) = meta
            else {
                bail!("Rust module path must be a literal string")
            };
            values.push(PathBuf::from(value.value()));
        }
        Ok(())
    }
    let mut values = Vec::new();
    for attr in attrs {
        paths(&attr.meta, &mut values, features)?;
    }
    ensure!(values.len() <= 1, "multiple Rust module path attributes");
    Ok(values.pop())
}

impl<'ast> Visit<'ast> for TestScanner<'_> {
    fn visit_block(&mut self, _: &'ast syn::Block) {}

    fn visit_item_fn(&mut self, item: &'ast syn::ItemFn) {
        let (candidate, rejection) = test_attributes(&item.attrs, &self.context.features);
        if !candidate {
            return;
        }
        let name = self
            .context
            .scope
            .iter()
            .map(String::as_str)
            .chain([item.sig.ident.to_string().as_str()])
            .collect::<Vec<_>>()
            .join("::");
        if let Some(reason) = self.context.rejection.clone().or(rejection) {
            if !self.index.tests.contains(&name) {
                self.index.rejected_tests.insert(name, reason);
            }
        } else {
            self.index.rejected_tests.remove(&name);
            self.index.tests.insert(name.clone());
            self.index
                .test_bodies
                .insert(name, item.block.to_token_stream().to_string());
        }
        // Nested functions and impl methods are not harness test functions.
    }

    fn visit_item_impl(&mut self, _: &'ast syn::ItemImpl) {}

    fn visit_item_mod(&mut self, item: &'ast syn::ItemMod) {
        let saved = self.context.clone();
        self.context.scope.push(item.ident.to_string());
        if self.context.rejection.is_none() {
            self.context.rejection = test_attributes(&item.attrs, &self.context.features).1;
        }
        let explicit = match module_path(&item.attrs, &self.context.features) {
            Ok(path) => path,
            Err(error) => {
                self.error = Some(error);
                self.context = saved;
                return;
            }
        };
        if item.content.is_some() {
            self.context.module_dir = explicit.as_ref().map_or_else(
                || saved.module_dir.join(item.ident.to_string()),
                |path| saved.path_dir.join(path),
            );
            self.context.path_dir = self.context.module_dir.clone();
            visit::visit_item_mod(self, item);
        } else {
            let path = if let Some(path) = explicit {
                Some(saved.path_dir.join(path))
            } else {
                let flat = saved.module_dir.join(format!("{}.rs", item.ident));
                let directory = saved.module_dir.join(item.ident.to_string()).join("mod.rs");
                match (flat.is_file(), directory.is_file()) {
                    (true, false) => Some(flat),
                    (false, true) => Some(directory),
                    (false, false) => None,
                    (true, true) => {
                        self.error = Some(anyhow::anyhow!(
                            "ambiguous Rust module source: {}",
                            item.ident
                        ));
                        None
                    }
                }
            };
            if let Some(path) = path.filter(|path| path.is_file()) {
                self.context.path = path.clone();
                self.context.path_dir = path.parent().map_or_else(PathBuf::new, Path::to_path_buf);
                self.context.module_dir = if path.file_name().is_some_and(|name| name == "mod.rs") {
                    self.context.path_dir.clone()
                } else {
                    self.context
                        .path_dir
                        .join(path.file_stem().unwrap_or_default())
                };
                self.queued.push(self.context.clone());
            }
        }
        self.context = saved;
    }
}

fn discovered_targets(crate_root: &Path, directory: &str) -> Result<BTreeMap<String, PathBuf>> {
    let directory = crate_root.join(directory);
    let mut targets = BTreeMap::new();
    if !directory.is_dir() {
        return Ok(targets);
    }
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        let source =
            if path.is_file() && path.extension().is_some_and(|extension| extension == "rs") {
                Some(path.clone())
            } else if path.is_dir() && path.join("main.rs").is_file() {
                Some(path.join("main.rs"))
            } else {
                None
            };
        if let Some(source) = source {
            let name = path
                .file_stem()
                .and_then(|name| name.to_str())
                .context("test target name must be UTF8")?;
            ensure!(
                targets.insert(name.into(), source).is_none(),
                "duplicate discovered target {name}"
            );
        }
    }
    Ok(targets)
}

fn target_rejection(table: &toml_edit::Table, context: &FeatureContext) -> Result<Option<String>> {
    if table.get("test").and_then(toml_edit::Item::as_bool) == Some(false) {
        return Ok(Some("Cargo target has test = false".into()));
    }
    if table.get("harness").and_then(toml_edit::Item::as_bool) == Some(false) {
        return Ok(Some("Cargo target has harness = false".into()));
    }
    if let Some(features) = table.get("required-features") {
        let features = features
            .as_array()
            .context("required-features must be an array")?;
        if features.iter().any(|feature| {
            feature.as_str().is_none_or(|name| {
                !context.declared.contains(name) || !context.enabled.contains(name)
            })
        }) {
            return Ok(Some(format!(
                "Cargo target requires features without registry feature context: {features}"
            )));
        }
    }
    Ok(None)
}

fn cargo_features(
    manifest: &toml_edit::DocumentMut,
    all_features: bool,
    requested: &BTreeSet<String>,
) -> Result<FeatureContext> {
    let mut definitions = BTreeMap::<String, Vec<String>>::new();
    if let Some(table) = manifest.get("features").and_then(toml_edit::Item::as_table) {
        for (name, values) in table.iter() {
            let values = values
                .as_array()
                .context("Cargo feature must be an array")?;
            definitions.insert(
                name.into(),
                values
                    .iter()
                    .map(|value| {
                        value
                            .as_str()
                            .map(str::to_owned)
                            .context("Cargo feature member must be a string")
                    })
                    .collect::<Result<_>>()?,
            );
        }
    }
    // Cargo declares an implicit feature for optional dependencies unless a
    // `dep:name` member explicitly suppresses that dependency's feature.
    let suppressed = definitions
        .values()
        .flatten()
        .filter_map(|value| value.strip_prefix("dep:"))
        .collect::<BTreeSet<_>>();
    let mut declared = definitions.keys().cloned().collect::<BTreeSet<_>>();
    for section in ["dependencies", "build-dependencies", "dev-dependencies"] {
        if let Some(table) = manifest.get(section).and_then(toml_edit::Item::as_table) {
            for (name, dependency) in table.iter() {
                if dependency
                    .get("optional")
                    .and_then(toml_edit::Item::as_bool)
                    == Some(true)
                    && !suppressed.contains(name)
                {
                    declared.insert(name.into());
                }
            }
        }
    }
    ensure!(
        requested.is_subset(&declared),
        "registry requests undeclared Cargo features: {:?}",
        requested.difference(&declared).collect::<Vec<_>>()
    );
    let mut enabled = if all_features {
        declared.clone()
    } else {
        requested.clone()
    };
    if declared.contains("default") {
        enabled.insert("default".into());
    }
    loop {
        let before = enabled.len();
        let inherited = enabled
            .iter()
            .filter_map(|name| definitions.get(name))
            .flatten()
            .filter(|name| declared.contains(*name))
            .cloned()
            .collect::<Vec<_>>();
        enabled.extend(inherited);
        if enabled.len() == before {
            break;
        }
    }
    Ok(FeatureContext { declared, enabled })
}

fn cargo_test_sources(
    crate_root: &Path,
    all_features: bool,
    requested: &BTreeMap<String, BTreeSet<String>>,
) -> Result<Vec<TestSource>> {
    let manifest: toml_edit::DocumentMut =
        fs::read_to_string(crate_root.join("Cargo.toml"))?.parse()?;
    let package = manifest["package"]["name"]
        .as_str()
        .context("crate package name missing")?
        .replace('-', "_");
    let features = cargo_features(
        &manifest,
        all_features,
        requested.get(&package).unwrap_or(&BTreeSet::new()),
    )?;
    let mut targets = BTreeMap::<(String, String), (PathBuf, Option<String>)>::new();
    let lib = manifest.get("lib").and_then(toml_edit::Item::as_table);
    if lib.is_some()
        || manifest["package"]
            .get("autolib")
            .and_then(toml_edit::Item::as_bool)
            != Some(false)
    {
        let path = crate_root.join(
            lib.and_then(|table| table.get("path"))
                .and_then(toml_edit::Item::as_str)
                .unwrap_or("src/lib.rs"),
        );
        if path.is_file() {
            targets.insert(
                ("lib".into(), "lib".into()),
                (
                    path,
                    lib.map(|table| target_rejection(table, &features))
                        .transpose()?
                        .flatten(),
                ),
            );
        }
    }
    for (kind, folder, auto) in [
        ("bin", "src/bin", "autobins"),
        ("test", "tests", "autotests"),
    ] {
        if manifest["package"]
            .get(auto)
            .and_then(toml_edit::Item::as_bool)
            != Some(false)
        {
            for (name, path) in discovered_targets(crate_root, folder)? {
                targets.insert((kind.into(), name), (path, None));
            }
            if kind == "bin" && crate_root.join("src/main.rs").is_file() {
                targets.insert(
                    (kind.into(), package.clone()),
                    (crate_root.join("src/main.rs"), None),
                );
            }
        }
        if let Some(declarations) = manifest
            .get(kind)
            .and_then(toml_edit::Item::as_array_of_tables)
        {
            for target in declarations {
                let name = target
                    .get("name")
                    .and_then(toml_edit::Item::as_str)
                    .context("Cargo test target name missing")?;
                let key = (kind.into(), name.to_owned());
                let path = if let Some(path) = target.get("path").and_then(toml_edit::Item::as_str)
                {
                    crate_root.join(path)
                } else if let Some((path, _)) = targets.get(&key) {
                    path.clone()
                } else {
                    crate_root.join(folder).join(format!("{name}.rs"))
                };
                ensure!(
                    path.is_file(),
                    "Cargo target source missing: {}",
                    path.display()
                );
                let canonical = path.canonicalize()?;
                // An explicitly named target owns its source path. Do not
                // accept an auto-discovered alias of a disabled/gated target.
                targets.retain(|(existing_kind, existing_name), (existing_path, _)| {
                    existing_kind != kind
                        || existing_name == name
                        || !existing_path
                            .canonicalize()
                            .is_ok_and(|source| source == canonical)
                });
                targets.insert(key, (path, target_rejection(target, &features)?));
            }
        }
    }
    Ok(targets
        .into_iter()
        .map(|((kind, name), (path, rejection))| {
            let mut scope = vec![package.clone()];
            if kind == "test"
                || (kind == "bin" && path.file_name().is_none_or(|name| name != "main.rs"))
            {
                scope.push(name.replace('-', "_"));
            }
            let directory = path.parent().map_or_else(PathBuf::new, Path::to_path_buf);
            TestSource {
                path,
                scope,
                module_dir: directory.clone(),
                path_dir: directory,
                rejection,
                features: features.clone(),
                ancestors: Vec::new(),
            }
        })
        .collect())
}

fn index_tests(
    root: &Path,
    index: &mut Index,
    all_features: bool,
    requested: &BTreeMap<String, BTreeSet<String>>,
) -> Result<()> {
    let workspace: toml_edit::DocumentMut = fs::read_to_string(root.join("Cargo.toml"))?.parse()?;
    let patterns = |key: &str| -> Result<Vec<String>> {
        workspace["workspace"]
            .get(key)
            .map(|item| {
                item.as_array()
                    .context("workspace membership must be an array")?
                    .iter()
                    .map(|pattern| {
                        pattern
                            .as_str()
                            .map(str::to_owned)
                            .context("workspace membership must be strings")
                    })
                    .collect()
            })
            .unwrap_or_else(|| Ok(Vec::new()))
    };
    let members = patterns("members")?;
    let excluded = patterns("exclude")?;
    let matches = |pattern: &str, relative: &str| {
        pattern == relative
            || pattern.strip_suffix("/*").is_some_and(|parent| {
                relative
                    .rsplit_once('/')
                    .is_some_and(|(directory, _)| directory == parent)
            })
    };
    let mut queued = Vec::new();
    let mut packages = BTreeSet::new();
    for entry in fs::read_dir(root.join("crates"))? {
        let path = entry?.path();
        if path.is_dir() && path.join("Cargo.toml").is_file() {
            let relative = path
                .strip_prefix(root)?
                .to_str()
                .context("crate path must be UTF8")?;
            if members.iter().any(|pattern| matches(pattern, relative))
                && !excluded.iter().any(|pattern| matches(pattern, relative))
            {
                let sources = cargo_test_sources(&path, all_features, requested)?;
                if let Some(source) = sources.first() {
                    packages.insert(source.scope[0].clone());
                }
                queued.extend(sources);
            }
        }
    }
    ensure!(
        requested.keys().all(|package| packages.contains(package)),
        "registry feature context names an unknown crate"
    );
    let canonical_root = root.canonicalize()?;
    let mut seen = BTreeSet::new();
    while let Some(mut context) = queued.pop() {
        context.path = context.path.canonicalize()?;
        ensure!(
            context.path.starts_with(&canonical_root),
            "test module source outside repository"
        );
        ensure!(
            !context.ancestors.contains(&context.path),
            "recursive Rust module inclusion"
        );
        if !seen.insert((
            context.path.clone(),
            context.scope.clone(),
            context.rejection.clone(),
        )) {
            continue;
        }
        ensure!(
            seen.len() <= 4096,
            "Rust test inclusion graph exceeds bound"
        );
        context.ancestors.push(context.path.clone());
        let source = fs::read_to_string(&context.path)?;
        ensure!(
            source.len() <= 32 * 1024 * 1024,
            "Rust test source exceeds byte bound"
        );
        let file = syn::parse_file(&source)
            .with_context(|| format!("index Rust test source {}", context.path.display()))?;
        if context.rejection.is_none() {
            context.rejection = test_attributes(&file.attrs, &context.features).1;
        }
        let mut scanner = TestScanner {
            index,
            context,
            queued: &mut queued,
            error: None,
        };
        scanner.visit_file(&file);
        if let Some(error) = scanner.error {
            return Err(error);
        }
    }
    Ok(())
}

pub(super) fn index(root: &Path) -> Result<Index> {
    index_with_features(root, &BTreeMap::new())
}

// Keys are normalized owning package names, such as `rtao_testkit`. Explicit
// features expand on top of that package's default features.
pub(super) fn index_with_features(
    root: &Path,
    requested: &BTreeMap<String, BTreeSet<String>>,
) -> Result<Index> {
    let mut files = Vec::new();
    super::rust_files(&root.join("crates"), &mut files)?;
    files.sort();
    let mut index = Index::default();
    for path in files {
        let relative = path
            .strip_prefix(root)?
            .to_str()
            .context("Rust path must be UTF8")?
            .replace('\\', "/");
        let parts = relative.split('/').collect::<Vec<_>>();
        let crate_name = parts
            .get(1)
            .context("crate source path malformed")?
            .replace('-', "_");
        let mut scope = vec![crate_name];
        let source_root = parts.get(2).copied().unwrap_or("");
        if source_root == "src" || source_root == "tests" {
            scope.extend(
                parts
                    .iter()
                    .skip(3)
                    .map(|part| part.trim_end_matches(".rs"))
                    .filter(|part| !matches!(*part, "lib" | "main" | "mod"))
                    .map(str::to_owned),
            );
        }
        let source = fs::read_to_string(&path)?;
        ensure!(
            source.len() <= 32 * 1024 * 1024,
            "Rust alias source exceeds byte bound"
        );
        let file =
            syn::parse_file(&source).with_context(|| format!("index Rust source {relative}"))?;
        let mut scanner = RustScanner {
            index: &mut index,
            path: relative,
            source: &source,
            scope,
            error: None,
        };
        scanner.visit_file(&file);
        if let Some(error) = scanner.error {
            return Err(error);
        }
    }
    index_tests(root, &mut index, false, requested)?;
    let mut feature_index = Index {
        rejected_tests: index.rejected_tests.clone(),
        ..Index::default()
    };
    // Required check lane: gates.rs runs nextest --workspace --all-features.
    index_tests(root, &mut feature_index, true, &BTreeMap::new())?;
    index.feature_tests = feature_index.tests;
    index.feature_test_bodies = feature_index.test_bodies;
    index.rejected_feature_tests = feature_index.rejected_tests;
    Ok(index)
}

fn names(expr: &ast::Expr, out: &mut Vec<String>) {
    match expr {
        ast::Expr::Name(name) => out.push(name.id.to_string()),
        ast::Expr::Tuple(tuple) => {
            for expr in &tuple.elts {
                names(expr, out)
            }
        }
        ast::Expr::List(list) => {
            for expr in &list.elts {
                names(expr, out)
            }
        }
        _ => {}
    }
}
fn canonical(source: &str, node: &impl Ranged) -> Result<Vec<String>> {
    let slice = &source[node.range().start().to_usize()..node.range().end().to_usize()];
    lexer::lex(slice, Mode::Module)
        .map(|item| {
            item.map(|(token, _)| format!("{token:?}"))
                .map_err(|error| anyhow::anyhow!("{error:?}"))
        })
        .collect()
}
fn collect(
    source: &str,
    suite: &[ast::Stmt],
    prefix: &str,
    out: &mut BTreeMap<String, Value>,
) -> Result<()> {
    for statement in suite {
        let mut symbols = Vec::new();
        match statement {
            ast::Stmt::FunctionDef(function) => symbols.push(function.name.to_string()),
            ast::Stmt::AsyncFunctionDef(function) => symbols.push(function.name.to_string()),
            ast::Stmt::ClassDef(class) => symbols.push(class.name.to_string()),
            ast::Stmt::Assign(assign) => {
                for target in &assign.targets {
                    names(target, &mut symbols)
                }
            }
            ast::Stmt::AnnAssign(assign) => names(&assign.target, &mut symbols),
            ast::Stmt::Import(import) => symbols.extend(
                import
                    .names
                    .iter()
                    .map(|alias| alias.asname.as_ref().unwrap_or(&alias.name).to_string()),
            ),
            ast::Stmt::ImportFrom(import) => symbols.extend(
                import
                    .names
                    .iter()
                    .map(|alias| alias.asname.as_ref().unwrap_or(&alias.name).to_string()),
            ),
            _ => {}
        }
        let decorators: &[ast::Expr] = match statement {
            ast::Stmt::ClassDef(class) => &class.decorator_list,
            ast::Stmt::FunctionDef(function) => &function.decorator_list,
            ast::Stmt::AsyncFunctionDef(function) => &function.decorator_list,
            _ => &[],
        };
        let mut tokens = canonical(source, statement)?;
        for decorator in decorators {
            tokens.push("Decorator".into());
            tokens.extend(canonical(source, decorator)?);
        }
        for symbol in symbols {
            let name = format!("{prefix}.{symbol}");
            out.insert(name,json!({"tokens":tokens,"line":source[..statement.range().start().to_usize()].bytes().filter(|byte|*byte==b'\n').count()+1}));
        }
        if let ast::Stmt::ClassDef(class) = statement {
            collect(
                source,
                &class.body,
                &format!("{prefix}.{}", class.name),
                out,
            )?;
        }
    }
    Ok(())
}
pub(super) fn module(path: &str) -> Result<String> {
    let path = path
        .strip_prefix("sdk/python/")
        .context("Python public module outside SDK")?
        .strip_suffix(".py")
        .context("Python path must end .py")?
        .trim_end_matches("/__init__");
    Ok(path.replace('/', "."))
}
pub(super) fn python_symbols(path: &str, source: &str) -> Result<BTreeMap<String, Value>> {
    let suite = ast::Suite::parse(source, path).map_err(|error| anyhow::anyhow!("{error}"))?;
    let mut out = BTreeMap::new();
    collect(source, &suite, &module(path)?, &mut out)?;
    Ok(out)
}

fn old_source(root: &Path, blob: &str) -> Result<String> {
    let cache = std::env::var_os("RTAO_UPSTREAM_CACHE")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("RTAO_TEMP_ROOT")
                .map(|root| std::path::PathBuf::from(root).join("pin/upstream.git"))
        })
        .unwrap_or_else(|| root.join("target/upstream.git"));
    let output = Command::new("git")
        .arg("-C")
        .arg(cache)
        .args(["cat-file", "blob", blob])
        .output()?;
    ensure!(
        output.status.success() && output.stdout.len() <= 8 * 1024 * 1024,
        "previous blessed Python blob {blob} unavailable offline; retain the pin cache to produce its structural report"
    );
    ensure!(
        crate::extract::git_blob(&output.stdout) == blob,
        "old Python source differs from its blessed blob"
    );
    String::from_utf8(output.stdout).context("old Python source is not UTF8")
}
pub(super) fn changed(
    root: &Path,
    rows: &[super::Row],
    tree: &BTreeMap<String, String>,
    index: &Index,
) -> Result<Vec<String>> {
    let mut report = Vec::new();
    for row in rows.iter().filter(|row| {
        row.py.ends_with(".py") && row.py.starts_with("sdk/python/") && row.synced_blob != "pending"
    }) {
        let Some(current) = tree.get(&row.py) else {
            continue;
        };
        if current == &row.synced_blob {
            continue;
        }
        let before = python_symbols(&row.py, &old_source(root, &row.synced_blob)?)?;
        let source = fs::read_to_string(root.join("vendor/upstream/src").join(&row.py))?;
        ensure!(
            crate::extract::git_blob(source.as_bytes()) == *current,
            "current Python source not authenticated"
        );
        let after = python_symbols(&row.py, &source)?;
        for name in before.keys().chain(after.keys()).collect::<BTreeSet<_>>() {
            if before.get(name).map(|row| &row["tokens"])
                == after.get(name).map(|row| &row["tokens"])
            {
                continue;
            }
            let counterparts = index
                .aliases
                .get(name)
                .map(|items| {
                    items
                        .iter()
                        .map(|item| format!("{}:{} ({})", item.path, item.line, item.item))
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_else(|| "no Rust counterpart".into());
            report.push(format!("Python {name} changed → {counterparts}"));
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn symbol_fingerprints_find_defaults_annotations_help_decorators_and_methods() -> Result<()> {
        let source = "@register\n@dataclass\nclass Example:\n    flag: bool = field(default=True, metadata={'help':'flag'})\n    @verify(mode='x')\n    def build(self, amount: int = 2):\n        return amount\n";
        let baseline = python_symbols("sdk/python/bittensor/intents/example.py", source)?;
        for changed in [
            source.replace("default=True", "default=False"),
            source.replace("bool =", "int ="),
            source.replace("'help':'flag'", "'help':'updated'"),
            source.replace("mode='x'", "mode='y'"),
            source.replace("int = 2", "int = 3"),
        ] {
            let after = python_symbols("sdk/python/bittensor/intents/example.py", &changed)?;
            assert_ne!(
                baseline["bittensor.intents.example.Example"]["tokens"],
                after["bittensor.intents.example.Example"]["tokens"]
            );
        }
        let whitespace = python_symbols(
            "sdk/python/bittensor/intents/example.py",
            &source.replace("True, metadata", "True ,  metadata"),
        )?;
        assert_eq!(baseline, whitespace);
        Ok(())
    }
    #[test]
    fn alias_and_test_index_use_parsed_attributes_instead_of_comment_text() -> Result<()> {
        let source = "// #[doc(alias = \"bittensor.fake\")]\n#[doc(alias = \"bittensor.real\")] pub struct Real;\nmod tests { #[test] fn pinned() {} fn helper() {} }";
        let file = syn::parse_file(source)?;
        let mut index = Index::default();
        let mut scanner = RustScanner {
            index: &mut index,
            path: "crates/example/src/lib.rs".into(),
            source,
            scope: vec!["example".into()],
            error: None,
        };
        scanner.visit_file(&file);
        let mut queued = Vec::new();
        let mut test_scanner = TestScanner {
            index: &mut index,
            context: TestSource {
                path: "crates/example/src/lib.rs".into(),
                scope: vec!["example".into()],
                module_dir: "crates/example/src".into(),
                path_dir: "crates/example/src".into(),
                rejection: None,
                features: FeatureContext::default(),
                ancestors: Vec::new(),
            },
            queued: &mut queued,
            error: None,
        };
        test_scanner.visit_file(&file);
        assert!(index.aliases.contains_key("bittensor.real"));
        assert!(!index.aliases.contains_key("bittensor.fake"));
        assert!(index.tests.contains("example::tests::pinned"));
        assert!(!index.tests.contains("example::tests::helper"));
        Ok(())
    }

    fn inline_test_index(source: &str) -> Result<Index> {
        let file = syn::parse_file(source)?;
        let mut index = Index::default();
        let mut queued = Vec::new();
        let mut scanner = TestScanner {
            index: &mut index,
            context: TestSource {
                path: "crates/example/src/lib.rs".into(),
                scope: vec!["example".into()],
                module_dir: "crates/example/src".into(),
                path_dir: "crates/example/src".into(),
                rejection: test_attributes(&file.attrs, &FeatureContext::default()).1,
                features: FeatureContext::default(),
                ancestors: Vec::new(),
            },
            queued: &mut queued,
            error: None,
        };
        scanner.visit_file(&file);
        if let Some(error) = scanner.error {
            return Err(error);
        }
        Ok(index)
    }

    #[test]
    fn ignored_and_conditionally_omitted_tests_cannot_supply_parity_evidence() -> Result<()> {
        let source = "#[cfg(test)] mod tests { #[test] fn evidence() { assert!(true); } }";
        let baseline = inline_test_index(source)?;
        assert!(baseline.tests.contains("example::tests::evidence"));
        assert!(
            baseline
                .test_bodies
                .contains_key("example::tests::evidence")
        );
        for changed in [
            source.replace("#[test]", "#[ignore] #[test]"),
            source.replace("#[test]", "#[ignore = \"later\"] #[test]"),
            source.replace("#[test]", "#[cfg(feature = \"optional\")] #[test]"),
            source.replace("#[test]", "#[cfg(unix)] #[test]"),
            source.replace("#[test]", "#[cfg(not(test))] #[test]"),
            source.replace("#[test]", "#[cfg_attr(test, ignore)] #[test]"),
            source.replace(
                "#[test]",
                "#[cfg_attr(feature = \"optional\", ignore)] #[test]",
            ),
            source.replace(
                "#[test]",
                "#[cfg_attr(feature = \"optional\", cfg(any()))] #[test]",
            ),
            source.replace("#[cfg(test)]", "#[cfg(feature = \"optional\")]"),
            source.replace(
                "#[cfg(test)]",
                "#[cfg_attr(feature = \"optional\", cfg(any()))]",
            ),
            format!("#![cfg(feature = \"optional\")]\n{source}"),
            format!("#![cfg_attr(test, ignore)]\n{source}"),
        ] {
            let after = inline_test_index(&changed)?;
            assert!(
                !after.tests.contains("example::tests::evidence"),
                "{changed}"
            );
            assert!(
                !after.test_bodies.contains_key("example::tests::evidence"),
                "{changed}"
            );
            assert!(
                after
                    .rejected_tests
                    .contains_key("example::tests::evidence"),
                "{changed}"
            );
        }
        for decoration in [
            "#[cfg(any(test, feature = \"optional\"))]",
            "#[cfg(all(test, not(any())))]",
            "#[cfg_attr(not(test), ignore)]",
            "#[cfg_attr(feature = \"optional\", allow(dead_code))]",
        ] {
            let changed = source.replace("#[test]", &format!("{decoration} #[test]"));
            assert!(
                inline_test_index(&changed)?
                    .tests
                    .contains("example::tests::evidence"),
                "{changed}"
            );
        }
        Ok(())
    }

    #[test]
    fn unsupported_markers_and_nested_items_cannot_supply_parity_evidence() -> Result<()> {
        let indexed = inline_test_index(
            "#[test] fn plain() {} #[tokio::test] async fn asynchronous() {} #[other::test] fn custom() {} #[rstest] fn parameterized() {} #[cfg_attr(feature = \"optional\", test)] fn conditional() {} fn helper() { #[test] fn nested() {} } const _: () = { #[test] fn nested_const() {} }; struct Example; impl Example { #[test] fn method() {} }",
        )?;
        assert_eq!(
            indexed.tests,
            BTreeSet::from(["example::plain".into(), "example::asynchronous".into()])
        );
        for rejected in ["custom", "parameterized", "conditional"] {
            assert!(
                indexed
                    .rejected_tests
                    .contains_key(&format!("example::{rejected}"))
            );
        }
        Ok(())
    }

    struct TestWorkspace {
        path: PathBuf,
    }

    impl TestWorkspace {
        fn new() -> Result<Self> {
            use std::sync::atomic::{AtomicU64, Ordering};
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "rtao-parity-test-index-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            ensure!(!path.exists(), "test-index temporary root already exists");
            fs::create_dir_all(path.join("crates/example/src"))?;
            let workspace = Self { path };
            workspace.write("Cargo.toml", "[workspace]\nmembers = ['crates/*']\n")?;
            workspace.write(
                "crates/example/Cargo.toml",
                "[package]\nname = 'example'\nversion = '0.1.0'\nedition = '2024'\n",
            )?;
            Ok(workspace)
        }

        fn write(&self, relative: &str, source: &str) -> Result<()> {
            let path = self.path.join(relative);
            fs::create_dir_all(path.parent().context("fixture path parent missing")?)?;
            fs::write(path, source)?;
            Ok(())
        }
    }

    impl Drop for TestWorkspace {
        fn drop(&mut self) {
            // Fixtures contain only files written above; they spawn no processes
            // and install no configuration, symlinks, or live dependencies.
            if let Err(error) = fs::remove_dir_all(&self.path) {
                eprintln!(
                    "could not remove test-index fixture {}: {error}",
                    self.path.display()
                );
            }
        }
    }

    #[test]
    fn cargo_targets_and_module_reachability_control_test_evidence() -> Result<()> {
        let workspace = TestWorkspace::new()?;
        workspace.write("crates/example/Cargo.toml", "[package]\nname = 'example'\nversion = '0.1.0'\nedition = '2024'\n[[test]]\nname = 'gated'\nrequired-features = ['docker']\n[[test]]\nname = 'disabled'\ntest = false\n[[test]]\nname = 'custom_harness'\nharness = false\n")?;
        workspace.write("crates/example/src/lib.rs", "#[cfg(test)] mod unit; #[cfg(feature = \"optional\")] mod gated; #[path = \"renamed.rs\"] mod explicit; mod inline { #[cfg_attr(feature = \"optional\", cfg(any()))] mod deeper; }")?;
        for relative in [
            "src/unit.rs",
            "src/gated.rs",
            "src/renamed.rs",
            "src/orphan.rs",
            "src/inline/deeper.rs",
            "tests/gated.rs",
            "tests/disabled.rs",
            "tests/custom_harness.rs",
            "tests/unconditional.rs",
        ] {
            workspace.write(
                &format!("crates/example/{relative}"),
                "#[test] fn evidence() { assert!(true); }",
            )?;
        }
        workspace.write(
            "crates/example/tests/file_gate.rs",
            "#![cfg(feature = \"optional\")]\n#[test] fn evidence() { assert!(true); }",
        )?;
        let indexed = index(&workspace.path)?;
        assert_eq!(
            indexed.tests,
            BTreeSet::from([
                "example::unit::evidence".into(),
                "example::explicit::evidence".into(),
                "example::unconditional::evidence".into()
            ])
        );
        for (name, reason) in [
            ("gated", "conditionally omitted"),
            ("inline::deeper", "conditional test attributes"),
            ("orphan", "not reachable"),
            ("file_gate", "conditionally omitted"),
            ("disabled", "test = false"),
            ("custom_harness", "harness = false"),
        ] {
            let name = format!("example::{name}::evidence");
            assert!(
                indexed
                    .rejected_tests
                    .get(&name)
                    .is_some_and(|rejection| rejection.contains(reason)),
                "{name}: {:?}",
                indexed.rejected_tests.get(&name)
            );
            assert!(!indexed.test_bodies.contains_key(&name));
        }
        // The target and source module share a name, so use a distinct target
        // to assert the Cargo rejection independently of the module rejection.
        workspace.write("crates/example/Cargo.toml", "[package]\nname = 'example'\nversion = '0.1.0'\nedition = '2024'\nautotests = false\n[[test]]\nname = 'gated_target'\npath = 'tests/gated.rs'\nrequired-features = ['docker']\n")?;
        let indexed = index(&workspace.path)?;
        assert!(!indexed.tests.contains("example::unconditional::evidence"));
        assert!(
            indexed.rejected_tests["example::gated_target::evidence"].contains("requires features")
        );
        assert!(
            indexed.rejected_tests["example::unconditional::evidence"].contains("not reachable")
        );
        workspace.write("crates/example/Cargo.toml", "[package]\nname = 'example'\nversion = '0.1.0'\nedition = '2024'\n[[test]]\nname = 'renamed_disabled'\npath = 'tests/unconditional.rs'\ntest = false\n")?;
        let indexed = index(&workspace.path)?;
        assert!(!indexed.tests.contains("example::unconditional::evidence"));
        assert!(
            indexed.rejected_tests["example::renamed_disabled::evidence"].contains("test = false")
        );
        workspace.write(
            "Cargo.toml",
            "[workspace]\nmembers = ['crates/*']\nexclude = ['crates/example']\n",
        )?;
        let indexed = index(&workspace.path)?;
        assert!(indexed.tests.is_empty());
        assert!(indexed.rejected_tests["example::unit::evidence"].contains("not reachable"));
        Ok(())
    }

    #[test]
    fn conditional_module_paths_cannot_replace_registered_evidence() -> Result<()> {
        let workspace = TestWorkspace::new()?;
        workspace.write(
            "crates/example/src/default.rs",
            "#[test] fn evidence() { assert!(DEFAULT); }",
        )?;
        workspace.write(
            "crates/example/src/alternate.rs",
            "#[test] fn evidence() { assert!(ALTERNATE); }",
        )?;
        for gate in [
            "#[cfg_attr(feature = \"optional\", path = \"alternate.rs\")]",
            "#[cfg_attr(feature = \"optional\", cfg_attr(test, path = \"alternate.rs\"))]",
            "#[cfg_attr(test, cfg_attr(feature = \"optional\", path = \"alternate.rs\"))]",
        ] {
            workspace.write("crates/example/src/lib.rs", &format!("{gate} mod default;"))?;
            let indexed = index(&workspace.path)?;
            assert!(
                !indexed.tests.contains("example::default::evidence"),
                "{gate}"
            );
            assert!(
                indexed.rejected_tests["example::default::evidence"]
                    .contains("conditional test attributes"),
                "{gate}"
            );
        }
        workspace.write(
            "crates/example/src/lib.rs",
            "#[cfg_attr(test, path = \"alternate.rs\")] mod default;",
        )?;
        let indexed = index(&workspace.path)?;
        assert!(indexed.tests.contains("example::default::evidence"));
        assert!(indexed.test_bodies["example::default::evidence"].contains("ALTERNATE"));
        assert!(!indexed.test_bodies["example::default::evidence"].contains("DEFAULT"));
        Ok(())
    }

    #[test]
    fn cargo_feature_context_expands_defaults_and_validates_declared_names() -> Result<()> {
        let manifest: toml_edit::DocumentMut = "[features]\ndefault = ['base', 'optional']\nbase = ['nested']\nnested = []\ndocker = ['dep:hidden']\n[dependencies]\noptional = {version = '1', optional = true}\nhidden = {version = '1', optional = true}\n".parse()?;
        let default = cargo_features(&manifest, false, &BTreeSet::new())?;
        assert_eq!(
            default.enabled,
            BTreeSet::from([
                "default".into(),
                "base".into(),
                "nested".into(),
                "optional".into()
            ])
        );
        assert!(default.declared.contains("docker"));
        assert!(!default.declared.contains("hidden"));
        let all = cargo_features(&manifest, true, &BTreeSet::new())?;
        assert!(all.enabled.contains("docker"));
        let explicit = cargo_features(&manifest, false, &BTreeSet::from(["docker".into()]))?;
        assert!(explicit.enabled.contains("docker"));
        assert!(explicit.enabled.contains("nested"));
        assert!(cargo_features(&manifest, false, &BTreeSet::from(["unknown".into()])).is_err());
        Ok(())
    }

    #[test]
    fn declared_test_features_qualify_without_loosening_other_source_gates() -> Result<()> {
        let workspace = TestWorkspace::new()?;
        workspace.write("crates/example/Cargo.toml", "[package]\nname = 'example'\nversion = '0.1.0'\nedition = '2024'\n[features]\ndefault = ['base']\nbase = ['nested']\nnested = []\ndocker = []\n[[test]]\nname = 'docker_target'\nrequired-features = ['docker']\n[[test]]\nname = 'unknown_target'\nrequired-features = ['undeclared']\n[[test]]\nname = 'default_target'\nrequired-features = ['nested']\n[[test]]\nname = 'disabled_target'\nrequired-features = ['docker']\ntest = false\n")?;
        workspace.write("crates/example/src/lib.rs", "#[cfg(feature = \"nested\")] mod baseline; #[cfg(feature = \"docker\")] mod docker; #[cfg(feature = \"undeclared\")] mod unknown; #[cfg(unix)] mod platform; #[cfg_attr(feature = \"docker\", path = \"alternate.rs\")] mod feature_path; #[test] #[cfg(not(feature = \"docker\"))] fn default_only() { assert!(true); }")?;
        for relative in [
            "src/baseline.rs",
            "src/unknown.rs",
            "src/platform.rs",
            "tests/docker_target.rs",
            "tests/unknown_target.rs",
            "tests/default_target.rs",
            "tests/disabled_target.rs",
        ] {
            workspace.write(
                &format!("crates/example/{relative}"),
                "#[test] fn evidence() { assert!(true); }",
            )?;
        }
        workspace.write("crates/example/src/docker.rs", "#[test] fn evidence() { assert!(true); } #[ignore] #[test] fn ignored() { assert!(true); }")?;
        workspace.write(
            "crates/example/src/feature_path.rs",
            "#[test] fn evidence() { assert!(DEFAULT); }",
        )?;
        workspace.write(
            "crates/example/src/alternate.rs",
            "#[test] fn evidence() { assert!(DOCKER); }",
        )?;
        let indexed = index(&workspace.path)?;
        assert!(indexed.tests.contains("example::baseline::evidence"));
        assert!(indexed.tests.contains("example::default_target::evidence"));
        assert!(indexed.tests.contains("example::default_only"));
        assert!(!indexed.tests.contains("example::docker::evidence"));
        assert!(!indexed.tests.contains("example::docker_target::evidence"));
        assert!(indexed.feature_tests.contains("example::docker::evidence"));
        assert!(
            indexed
                .feature_tests
                .contains("example::docker_target::evidence")
        );
        assert!(!indexed.feature_tests.contains("example::default_only"));
        assert!(indexed.test_bodies["example::feature_path::evidence"].contains("DEFAULT"));
        assert!(indexed.feature_test_bodies["example::feature_path::evidence"].contains("DOCKER"));
        for rejected in [
            "unknown::evidence",
            "unknown_target::evidence",
            "platform::evidence",
            "disabled_target::evidence",
            "docker::ignored",
        ] {
            let name = format!("example::{rejected}");
            assert!(!indexed.feature_tests.contains(&name), "{name}");
            assert!(!indexed.feature_test_bodies.contains_key(&name), "{name}");
            assert!(indexed.rejected_feature_tests.contains_key(&name), "{name}");
        }
        let explicit = index_with_features(
            &workspace.path,
            &BTreeMap::from([("example".into(), BTreeSet::from(["docker".into()]))]),
        )?;
        assert!(explicit.tests.contains("example::docker::evidence"));
        assert!(explicit.tests.contains("example::docker_target::evidence"));
        assert!(!explicit.tests.contains("example::default_only"));
        assert!(
            index_with_features(
                &workspace.path,
                &BTreeMap::from([("example".into(), BTreeSet::from(["undeclared".into()]))])
            )
            .is_err()
        );
        Ok(())
    }
}
