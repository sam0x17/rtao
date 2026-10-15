// Derived from RaoFoundation/subtensor sdk/python/tests/unit/test_facades.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Source completeness for the compiler-checked blocking facade witnesses.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use anyhow::{Context, Result, ensure};
use quote::ToTokens;
use rustpython_parser::{Parse as _, ast};
use syn::{
    Token,
    parse::{Parse, ParseStream},
    punctuated::Punctuated,
};

struct Contract {
    test: syn::Ident,
    rows: BTreeMap<String, Vec<String>>,
}

impl Parse for Contract {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let test: syn::Ident = input.parse()?;
        input.parse::<Token![;]>()?;
        let mut rows = BTreeMap::new();
        while !input.is_empty() {
            let attrs = input.call(syn::Attribute::parse_outer)?;
            let name: syn::LitStr = input.parse()?;
            let params;
            syn::bracketed!(params in input);
            let params = Punctuated::<syn::LitStr, Token![,]>::parse_terminated(&params)?;
            input.parse::<Token![=>]>()?;
            let method: syn::ExprPath = input.parse()?;
            input.parse::<Token![:]>()?;
            let signature: syn::TypeBareFn = input.parse()?;
            input.parse::<Token![;]>()?;
            let fail = |message: &str| syn::Error::new(name.span(), message);
            let class = match test.to_string().as_str() {
                "client_contract" => "SyncClient",
                "snapshot_contract" => "SyncSnapshot",
                _ => return Err(fail("unknown facade contract")),
            };
            if method.qself.is_some()
                || method.path.segments.len() != 2
                || method.path.segments[0].ident != class
                || method.path.segments[1].ident != name.value()
            {
                return Err(fail(
                    "contract must use the matching native blocking method",
                ));
            }
            let cfg = attrs
                .iter()
                .map(|attr| attr.to_token_stream().to_string())
                .collect::<Vec<_>>();
            let expected_cfg = if name.value() == "mine_pow_registration" {
                vec![quote::quote!(#[cfg(feature = "pow")]).to_string()]
            } else {
                Vec::new()
            };
            if cfg != expected_cfg || signature.unsafety.is_some() {
                return Err(fail("contract must compile as a safe blocking function"));
            }
            if rows
                .insert(
                    name.value(),
                    params.iter().map(syn::LitStr::value).collect(),
                )
                .is_some()
            {
                return Err(fail("duplicate facade method witness"));
            }
        }
        Ok(Self { test, rows })
    }
}

fn python_surface(source: &str, class_name: &str) -> Result<BTreeMap<String, Vec<String>>> {
    let suite = ast::Suite::parse(source, class_name)?;
    let class = suite
        .iter()
        .find_map(|stmt| match stmt {
            ast::Stmt::ClassDef(class) if class.name.as_str() == class_name => Some(class),
            _ => None,
        })
        .context("pinned facade class missing")?;
    ensure!(
        class.bases.is_empty(),
        "facade inheritance requires an explicit surface review"
    );
    let mut methods = BTreeMap::new();
    for stmt in &class.body {
        let ast::Stmt::AsyncFunctionDef(function) = stmt else {
            continue;
        };
        if function.name.as_str().starts_with('_') {
            continue;
        }
        let args = &function.args;
        let mut params = args
            .posonlyargs
            .iter()
            .chain(&args.args)
            .map(|arg| arg.def.arg.to_string())
            .filter(|arg| arg != "self")
            .collect::<Vec<_>>();
        params.extend(args.vararg.iter().map(|arg| arg.arg.to_string()));
        params.extend(args.kwonlyargs.iter().map(|arg| arg.def.arg.to_string()));
        params.extend(args.kwarg.iter().map(|arg| arg.arg.to_string()));
        ensure!(
            methods.insert(function.name.to_string(), params).is_none(),
            "duplicate Python method"
        );
    }
    ensure!(
        !methods.is_empty(),
        "pinned facade must expose async operations"
    );
    Ok(methods)
}

// pyt: unit/test_facades.py::test_sync_facade_mirrors_async_surface
#[test]
fn every_pinned_async_operation_has_a_typed_blocking_witness() -> Result<()> {
    let root = super::super::root()?;
    let pins = super::super::read_document(&root, "upstream.toml")?;
    let witness = syn::parse_file(&std::fs::read_to_string(
        root.join("crates/rtao/tests/facade_contract.rs"),
    )?)?;
    let mut contracts = BTreeMap::new();
    for item in witness.items {
        if let syn::Item::Macro(item) = item
            && item.mac.path.is_ident("facade_contract")
        {
            let contract: Contract = syn::parse2(item.mac.tokens)?;
            ensure!(
                contracts
                    .insert(contract.test.to_string(), contract.rows)
                    .is_none(),
                "duplicate facade contract"
            );
        }
    }
    ensure!(contracts.len() == 2, "both facade contracts are required");
    for (class, file, contract) in [
        ("Client", "client.py", "client_contract"),
        ("Snapshot", "snapshot.py", "snapshot_contract"),
    ] {
        let source = String::from_utf8(super::input(
            &root,
            &pins,
            &format!("sdk/python/bittensor/{file}"),
        )?)?;
        let expected = python_surface(&source, class)?;
        ensure!(
            contracts.get(contract) == Some(&expected),
            "{class} async surface differs from its typed blocking witnesses"
        );
    }
    Ok(())
}

#[test]
fn surface_discovery_includes_async_generators_and_all_parameter_kinds() -> Result<()> {
    let source = "class Client:\n    async def blocks(self, /, name, *items, flag=None, **kwargs):\n        yield name\n    async def _private(self):\n        pass\n    def pure(self):\n        pass\n";
    ensure!(
        python_surface(source, "Client")?
            == BTreeMap::from([(
                "blocks".into(),
                vec![
                    "name".into(),
                    "items".into(),
                    "flag".into(),
                    "kwargs".into()
                ]
            )])
    );
    ensure!(
        python_surface(
            "class Client(Base):\n    async def read(self): pass\n",
            "Client"
        )
        .is_err()
    );
    Ok(())
}

fn rust_files(directory: &Path, files: &mut Vec<std::path::PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_dir() {
            rust_files(&entry.path(), files)?;
        } else if kind.is_file() && entry.path().extension().is_some_and(|ext| ext == "rs") {
            files.push(entry.path());
        }
    }
    Ok(())
}

type Methods = BTreeMap<(String, String), Vec<String>>;

fn namespace_methods(
    items: &[syn::Item],
    classes: &BTreeSet<String>,
    asynchronous: bool,
    methods: &mut Methods,
) -> Result<()> {
    for item in items {
        if let syn::Item::Mod(module) = item {
            // Test-only local types do not add public SDK operations.
            if !module.attrs.iter().any(|attr| {
                attr.path().is_ident("cfg")
                    && attr.meta.to_token_stream().to_string() == "cfg (test)"
            }) && let Some((_, items)) = &module.content
            {
                namespace_methods(items, classes, asynchronous, methods)?;
            }
            continue;
        }
        let syn::Item::Impl(item) = item else {
            continue;
        };
        let syn::Type::Path(target) = item.self_ty.as_ref() else {
            continue;
        };
        let Some(segment) = target.path.segments.last() else {
            continue;
        };
        let raw_class = segment.ident.to_string();
        let class = if asynchronous {
            raw_class.as_str()
        } else {
            raw_class.strip_prefix("Sync").unwrap_or("")
        };
        if !classes.contains(class) {
            continue;
        }
        let extension = item.trait_.as_ref().is_some_and(|(_, path, _)| {
            path.segments
                .last()
                .is_some_and(|segment| segment.ident == "SubnetsMetagraphExt")
        });
        for member in &item.items {
            let syn::ImplItem::Fn(method) = member else {
                continue;
            };
            if !matches!(method.vis, syn::Visibility::Public(_)) && !extension {
                continue;
            }
            let signature = &method.sig;
            let name = signature.ident.to_string();
            if asynchronous && matches!(name.as_str(), "new" | "view") {
                ensure!(
                    signature.constness.is_some() && signature.asyncness.is_none(),
                    "{class}.{name} must remain a pure namespace binding helper"
                );
                continue;
            }
            ensure!(
                signature.asyncness.is_some() == asynchronous,
                "{class}.{name} changed its async/blocking kind"
            );
            let mut params = Vec::new();
            for arg in &signature.inputs {
                if let syn::FnArg::Typed(arg) = arg {
                    let syn::Pat::Ident(name) = arg.pat.as_ref() else {
                        anyhow::bail!("namespace argument needs explicit correspondence");
                    };
                    params.push(name.ident.to_string());
                }
            }
            ensure!(
                methods
                    .insert((class.into(), name.clone()), params)
                    .is_none(),
                "duplicate {class}.{name} namespace operation"
            );
        }
    }
    Ok(())
}

// pyt: unit/test_facades.py::test_namespace_reads_are_coroutines
// Constructors/accessors are native pure plumbing. Every read, including
// handwritten aliases and the metagraph extension, must have a blocking twin
// with exactly the same ordered parameter names. Owned Vec replaces the
// metagraph's borrowed field slice; the compiler checks that delegation.
#[test]
fn namespace_read_methods_are_async_and_have_blocking_twins() -> Result<()> {
    let root = super::super::root()?;
    let pins = super::super::read_document(&root, "upstream.toml")?;
    let source = String::from_utf8(super::input(&root, &pins, super::PYTHON)?)?;
    let catalog = serde_json::from_slice(&super::input(&root, &pins, super::CATALOG)?)?;
    let classes = super::discover(&source, &catalog)?
        .into_iter()
        .map(|namespace| namespace.class)
        .collect();
    let mut files = Vec::new();
    rust_files(&root.join("crates/rtao-reads/src"), &mut files)?;
    rust_files(&root.join("crates/rtao-metagraph/src"), &mut files)?;
    let mut asynchronous = Methods::new();
    for file in files {
        let source = std::fs::read_to_string(&file)?;
        let file = syn::parse_file(&source)?;
        namespace_methods(&file.items, &classes, true, &mut asynchronous)?;
    }
    let mut blocking = Methods::new();
    for file in [
        "crates/rtao/src/generated/blocking_namespaces.rs",
        "crates/rtao/src/blocking.rs",
    ] {
        let file = syn::parse_file(&std::fs::read_to_string(root.join(file))?)?;
        namespace_methods(&file.items, &classes, false, &mut blocking)?;
    }
    ensure!(
        !asynchronous.is_empty(),
        "namespace read surface cannot be empty"
    );
    ensure!(
        asynchronous == blocking,
        "namespace read operations or ordered parameters differ from their blocking twins"
    );
    Ok(())
}
