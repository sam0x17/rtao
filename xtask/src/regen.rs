//! Regenerate the name-based chain surface from immutable pinned V15/V16 inputs.

#[path = "regen_basket_index.rs"]
mod basket_index;
#[path = "regen_crosscheck.rs"]
mod crosscheck;
#[path = "regen_emit.rs"]
pub(crate) mod emit;
#[path = "regen_intents.rs"]
mod intents;
#[path = "regen_models.rs"]
mod models;
#[path = "regen_namespaces.rs"]
mod namespaces;
#[path = "regen_reads.rs"]
mod reads;
pub(crate) use rtao_runtime::shape;
#[path = "regen_types.rs"]
pub(crate) mod types;

use anyhow::{Context, Result, bail, ensure};
use frame_metadata::{RuntimeMetadata, RuntimeMetadataPrefixed, v16};
use parity_scale_codec::{DecodeAll, Encode};
use rtao_core::{ir::MetadataIr, runtime::Runtime};
use shape::{OuterEnums, ShapeHash};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
use toml_edit::DocumentMut;

pub(crate) fn shape_parts(parts: &[(&str, ShapeHash)]) -> ShapeHash {
    use blake2::{Blake2b, Digest, digest::consts::U32};
    ShapeHash(
        Blake2b::<U32>::digest(
            parts
                .iter()
                .map(|(name, shape)| (*name, shape.0))
                .collect::<Vec<_>>()
                .encode(),
        )
        .into(),
    )
}
fn root() -> Result<PathBuf> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_owned)
        .context("xtask has no workspace root")
}
fn read_document(root: &Path, name: &str) -> Result<DocumentMut> {
    fs::read_to_string(root.join(name))
        .with_context(|| format!("read {name}"))?
        .parse()
        .with_context(|| format!("parse {name}"))
}
fn string_array(document: &DocumentMut, name: &str) -> Result<Vec<String>> {
    document
        .get(name)
        .and_then(|v| v.as_array())
        .with_context(|| format!("scope {name} missing"))?
        .iter()
        .map(|v| {
            v.as_str()
                .map(str::to_owned)
                .context("scope item is not string")
        })
        .collect()
}
fn format_rust(source: &str) -> Result<String> {
    let mut child = Command::new("rustfmt")
        .args(["--edition", "2024", "--emit", "stdout"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("start rustfmt")?;
    child
        .stdin
        .take()
        .context("rustfmt stdin")?
        .write_all(source.as_bytes())?;
    let output = child.wait_with_output()?;
    ensure!(
        output.status.success(),
        "generated Rust invalid: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).context("rustfmt output UTF-8")
}
fn deprecations(metadata: &v16::RuntimeMetadataV16) -> BTreeMap<(String, u8), String> {
    let mut map = BTreeMap::new();
    for pallet in &metadata.pallets {
        if let Some(calls) = &pallet.calls {
            for (index, info) in &calls.deprecation_info.0 {
                let note = match info {
                    v16::VariantDeprecationInfo::DeprecatedWithoutNote => {
                        "Deprecated by runtime metadata".to_owned()
                    }
                    v16::VariantDeprecationInfo::Deprecated { note, since } => format!(
                        "{note}{}",
                        since
                            .as_ref()
                            .map(|v| format!(" (since {v})"))
                            .unwrap_or_default()
                    ),
                };
                map.insert((pallet.name.clone(), *index), note);
            }
        }
    }
    map
}
fn selective(source: &str) -> Result<String> {
    let file = syn::parse_file(source).context("parse pinned selective source")?;
    let enumeration = file
        .items
        .iter()
        .find_map(|item| match item {
            syn::Item::Enum(item) if item.ident == "SelectiveMetagraphIndex" => Some(item),
            _ => None,
        })
        .context("SelectiveMetagraphIndex missing")?;
    let mut variants = Vec::new();
    for (index, variant) in enumeration.variants.iter().enumerate() {
        ensure!(
            variant.fields.is_empty() && variant.discriminant.is_none(),
            "selective enum no longer implicit unit variants"
        );
        variants.push((u8::try_from(index)?, variant.ident.to_string()));
    }
    ensure!(!variants.is_empty(), "empty selective index");
    let implementation=file.items.iter().find_map(|item|match item {syn::Item::Impl(item) if quote::quote!(#item).to_string().contains("SelectiveMetagraphIndex")&&item.items.iter().any(|item|matches!(item,syn::ImplItem::Fn(method) if method.sig.ident=="from_index"))=>Some(item),_=>None}).context("selective from_index missing")?;
    let method = implementation
        .items
        .iter()
        .find_map(|item| match item {
            syn::ImplItem::Fn(method) if method.sig.ident == "from_index" => Some(method),
            _ => None,
        })
        .context("selective method missing")?;
    let matching = method
        .block
        .stmts
        .iter()
        .find_map(|stmt| match stmt {
            syn::Stmt::Expr(syn::Expr::Match(expr), _) => Some(expr),
            _ => None,
        })
        .context("selective from_index is not literal match")?;
    let mut actual = BTreeMap::new();
    for arm in &matching.arms {
        if let syn::Pat::Lit(syn::ExprLit {
            lit: syn::Lit::Int(index),
            ..
        }) = &arm.pat
        {
            let syn::Expr::Call(call) = arm.body.as_ref() else {
                bail!("selective arm is not Some variant")
            };
            ensure!(
                quote::quote!(#call.func)
                    .to_string()
                    .replace(' ', "")
                    .contains("Some"),
                "selective arm is not Some"
            );
            let Some(syn::Expr::Path(path)) = call.args.first() else {
                bail!("selective arm variant invalid")
            };
            let variant = path
                .path
                .segments
                .last()
                .context("selective variant absent")?
                .ident
                .to_string();
            ensure!(
                actual
                    .insert(index.base10_parse::<u8>()?, variant)
                    .is_none(),
                "duplicate selective source index"
            );
        }
    }
    ensure!(
        actual == variants.iter().cloned().collect(),
        "selective enum and from_index source differ"
    );
    let mut out = String::from(
        "/// Selective metagraph field indices extracted from pinned pallet source.\n#[derive(Clone,Copy,Debug,PartialEq,Eq,PartialOrd,Ord,Hash)]\n#[repr(u8)]\npub enum SelectiveMetagraphIndex {\n",
    );
    for (index, name) in &variants {
        out.push_str(&format!(
            "/// Select the `{}` field.\n{name} = {index},\n",
            emit::snake(name)
        ));
    }
    out.push_str("}\nimpl SelectiveMetagraphIndex {\n/// Resolve one supported selective field index.\npub const fn from_index(index:u8)->Option<Self> { match index {\n");
    for (index, name) in &variants {
        out.push_str(&format!("{index} => Some(Self::{name}),\n"));
    }
    out.push_str("_=>None,\n}}\n/// Return the wire field index.\npub const fn index(self)->u8 {self as u8}\n}\n");
    Ok(out)
}

pub(super) fn run(args: Vec<String>) -> Result<()> {
    run_at(args, &root()?)
}

/// Recompute metadata-generated outputs against an explicit workspace root.
pub(crate) fn check_at(root: &Path) -> Result<()> {
    run_at(vec!["--check".into()], root)
}

fn run_at(args: Vec<String>, root_path: &Path) -> Result<()> {
    let root = root_path.to_owned();
    ensure!(
        args.iter().all(|arg| matches!(
            arg.as_str(),
            "--check" | "--models-only" | "--namespaces-only" | "--intents-only" | "--reads-only"
        )),
        "usage: cargo xtask regen [--check] [--models-only|--namespaces-only|--intents-only|--reads-only]"
    );
    let models_only = args.iter().any(|arg| arg == "--models-only");
    let namespaces_only = args.iter().any(|arg| arg == "--namespaces-only");
    let intents_only = args.iter().any(|arg| arg == "--intents-only");
    ensure!(
        args.iter()
            .filter(|arg| matches!(
                arg.as_str(),
                "--models-only" | "--namespaces-only" | "--intents-only" | "--reads-only"
            ))
            .count()
            <= 1,
        "select only one scoped regeneration mode"
    );
    let check = models_only || args.iter().any(|arg| arg == "--check");
    let pins = read_document(&root, "upstream.toml")?;
    crate::metadata_pin::verify(&root, &pins)?;
    if namespaces_only {
        return namespaces::run(&root, &pins, check);
    }
    if args.iter().any(|arg| arg == "--reads-only") {
        return reads::run(&root, check);
    }
    let finney = pins
        .get("networks")
        .and_then(|v| v.get("finney"))
        .context("finney pin missing")?;
    let spec = u32::try_from(
        finney
            .get("metadata")
            .and_then(|v| v.get("spec"))
            .and_then(|v| v.as_integer())
            .context("finney spec missing")?,
    )?;
    let pin = finney.get("metadata").context("metadata pin missing")?;
    let blob = |name| -> Result<Vec<u8>> {
        let path = pin
            .get(name)
            .and_then(|v| v.as_str())
            .context("metadata path missing")?;
        let path = root.join(path);
        ensure!(
            fs::symlink_metadata(&path)?.file_type().is_file(),
            "metadata must be regular file"
        );
        let bytes = fs::read(path)?;
        ensure!(
            bytes.len() <= 16 * 1024 * 1024,
            "metadata size exceeds bound"
        );
        Ok(bytes)
    };
    let v15 = blob("v15")?;
    let v16 = blob("v16")?;
    let metadata15 = RuntimeMetadataPrefixed::decode_all(&mut &v15[..])?;
    let RuntimeMetadata::V15(metadata15) = metadata15.1 else {
        bail!("V15 pin is not V15")
    };
    let metadata16 = RuntimeMetadataPrefixed::decode_all(&mut &v16[..])?;
    let RuntimeMetadata::V16(metadata16) = metadata16.1 else {
        bail!("V16 pin is not V16")
    };
    let runtime = Runtime::parse(&v15, spec, 1, 42)?;
    let mut ir = MetadataIr::from_runtime(&runtime)?;
    ir.set_v16_api_versions(&metadata16)?;
    let scope_document = read_document(&root, "codegen/scope.toml")?;
    let covered = string_array(&scope_document, "covered_pallets")?;
    let mut scope: BTreeSet<String> = covered.iter().cloned().collect();
    scope.extend(string_array(&scope_document, "extra_pallets")?);
    ensure!(
        scope_document.get("runtime_apis").and_then(|v| v.as_str()) == Some("all"),
        "runtime API scope must be explicit all"
    );
    for name in &scope {
        ensure!(
            ir.pallets.iter().any(|p| &p.name == name),
            "configured pallet {name} absent"
        );
    }
    let mapper = types::TypeMapper::new(
        &runtime,
        &read_document(&root, "codegen/types.toml")?,
        &read_document(&root, "codegen/models.toml")?,
        &read_document(&root, "codegen/overrides.toml")?,
    )?;
    mapper.validate_prices(&ir, &scope)?;
    if intents_only {
        return intents::run(&root, &ir, &mapper, check);
    }
    models::check(&root, &mapper)?;
    models::freeze_tripwires(&root, check)?;
    if models_only {
        println!("hand models and frozen source tripwires match pinned metadata");
        return Ok(());
    }
    crosscheck::check(&root, &ir, &covered)?;
    let outer = OuterEnums {
        call: metadata15.outer_enums.call_enum_ty.id,
        event: metadata15.outer_enums.event_enum_ty.id,
        error: metadata15.outer_enums.error_enum_ty.id,
    };
    let mut bodies = emit::generate(
        &runtime,
        &ir,
        &mapper,
        &scope,
        outer,
        &deprecations(&metadata16),
    )?;
    bodies.get_mut("calls").context("calls body missing")?.insert_str(0,"// Metadata preserves the public call argument order and arity.\n#![allow(clippy::too_many_arguments)]\n");
    for name in ["runtime_apis", "storage"] {
        bodies.get_mut(name).context("descriptor body missing")?.insert_str(0,"// Signatures preserve metadata container and tuple types.\n#![allow(clippy::type_complexity)]\n");
    }
    bodies.insert(
        "selective".to_owned(),
        selective(&fs::read_to_string(root.join(
            "vendor/upstream/src/pallets/subtensor/src/rpc_info/metagraph.rs",
        ))?)?,
    );
    bodies.insert("mod".to_owned(), "//! Generated chain surface.\n/// Typed call builders and their exact argument contracts.\npub mod calls;\n/// Typed constant descriptors.\npub mod constants;\n/// Complete module error catalogue.\npub mod errors;\n/// Indexed event descriptors.\npub mod events;\n/// Expected signed extensions.\npub mod extensions;\n/// Typed runtime API descriptors.\npub mod runtime_apis;\n/// Selective metagraph indices.\npub mod selective;\n/// Generated shape verification inventory.\npub mod shapes;\n/// Name-based storage descriptors.\npub mod storage;\npub use shapes::PALLETS;\n".to_owned());
    let mut formatted = BTreeMap::new();
    for (name, body) in bodies {
        formatted.insert(name, format_rust(&body)?);
    }
    let mut fingerprint = blake3::Hasher::new();
    for (name, body) in &formatted {
        fingerprint.update(name.as_bytes());
        fingerprint.update(body.as_bytes());
    }
    let header = format!(
        "// @generated by `cargo xtask regen`; do not edit.\n// Surface hash b3:{} (System.Version excluded).\n",
        fingerprint.finalize().to_hex()
    );
    let directory = root.join("crates/rtao-runtime/src/generated");
    if check {
        ensure!(
            directory.exists(),
            "generated runtime directory missing; run cargo xtask regen"
        );
    } else {
        fs::create_dir_all(&directory)?;
    }
    ensure!(
        fs::symlink_metadata(&directory)?.file_type().is_dir(),
        "generated output directory must not be a symlink"
    );
    let expected: BTreeSet<_> = formatted.keys().map(|name| format!("{name}.rs")).collect();
    for entry in fs::read_dir(&directory)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        ensure!(expected.contains(&name), "orphan generated file {name}");
    }
    let mut differences = Vec::new();
    for (name, body) in formatted {
        let path = directory.join(format!("{name}.rs"));
        let attribution = if name == "selective" {
            let commit = pins
                .get("parity")
                .and_then(|item| item.get("commit"))
                .and_then(|item| item.as_str())
                .context("parity commit missing")?;
            format!(
                "// Derived from RaoFoundation/subtensor pallets/subtensor/src/rpc_info/metagraph.rs @ {commit} (Apache-2.0); modified by rtao.\n"
            )
        } else {
            String::new()
        };
        let output = format!("{attribution}{header}{body}");
        if path.exists() {
            ensure!(
                fs::symlink_metadata(&path)?.file_type().is_file(),
                "generated output must be regular"
            );
        }
        if fs::read_to_string(&path).ok().as_deref() != Some(&output) {
            if check {
                differences.push(name);
            } else {
                fs::write(path, output)?;
            }
        }
    }
    ensure!(
        differences.is_empty(),
        "generated runtime drift: {}; run cargo xtask regen",
        differences.join(", ")
    );
    println!(
        "runtime descriptors {} ({} scoped pallets, {} APIs)",
        if check {
            "match pinned metadata"
        } else {
            "regenerated"
        },
        scope.len(),
        ir.runtime_apis.len()
    );
    if !models_only {
        namespaces::run(&root, &pins, check)?;
        reads::run(&root, check)?;
        intents::run(&root, &ir, &mapper, check)?;
        basket_index::run(&root, &pins, check)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    fn fixture() -> (Runtime, MetadataIr, OuterEnums, BTreeSet<String>) {
        let root = root().unwrap();
        let raw = fs::read(root.join("vendor/metadata/finney/478.v15.scale")).unwrap();
        let metadata = RuntimeMetadataPrefixed::decode_all(&mut &raw[..]).unwrap();
        let RuntimeMetadata::V15(metadata) = metadata.1 else {
            unreachable!()
        };
        let raw16 = fs::read(root.join("vendor/metadata/finney/478.v16.scale")).unwrap();
        let metadata16 = RuntimeMetadataPrefixed::decode_all(&mut &raw16[..]).unwrap();
        let RuntimeMetadata::V16(metadata16) = metadata16.1 else {
            unreachable!()
        };
        let runtime = Runtime::parse(&raw, 478, 1, 42).unwrap();
        let mut ir = MetadataIr::from_runtime(&runtime).unwrap();
        ir.set_v16_api_versions(&metadata16).unwrap();
        let outer = OuterEnums {
            call: metadata.outer_enums.call_enum_ty.id,
            event: metadata.outer_enums.event_enum_ty.id,
            error: metadata.outer_enums.error_enum_ty.id,
        };
        let scope = read_document(&root, "codegen/scope.toml").unwrap();
        let mut names: BTreeSet<_> = string_array(&scope, "covered_pallets")
            .unwrap()
            .into_iter()
            .collect();
        names.extend(string_array(&scope, "extra_pallets").unwrap());
        (runtime, ir, outer, names)
    }
    fn mapper(runtime: &Runtime) -> types::TypeMapper<'_> {
        let root = root().unwrap();
        types::TypeMapper::new(
            runtime,
            &read_document(&root, "codegen/types.toml").unwrap(),
            &read_document(&root, "codegen/models.toml").unwrap(),
            &read_document(&root, "codegen/overrides.toml").unwrap(),
        )
        .unwrap()
    }
    #[test]
    fn changing_only_spec_and_system_version_produces_identical_descriptors() {
        let (runtime, mut ir, outer, scope) = fixture();
        let mapper = mapper(&runtime);
        let before =
            emit::generate(&runtime, &ir, &mapper, &scope, outer, &BTreeMap::new()).unwrap();
        ir.spec_version += 1;
        let version = ir
            .pallets
            .iter_mut()
            .find(|p| p.name == "System")
            .unwrap()
            .constant_details
            .iter_mut()
            .find(|c| c.name == "Version")
            .unwrap();
        version.value.fill(0xff);
        let after =
            emit::generate(&runtime, &ir, &mapper, &scope, outer, &BTreeMap::new()).unwrap();
        assert_eq!(before, after);
    }
    #[test]
    fn price_gate_requires_every_semantic_override_and_refuses_unreviewed_additions() {
        let (runtime, mut ir, _, scope) = fixture();
        let mut mapper = mapper(&runtime);
        mapper.validate_prices(&ir, &scope).unwrap();
        mapper.arguments.remove(&(
            "SubtensorModule".to_owned(),
            "add_stake_burn".to_owned(),
            "limit".to_owned(),
        ));
        assert!(
            mapper
                .validate_prices(&ir, &scope)
                .unwrap_err()
                .to_string()
                .contains("add_stake_burn.limit")
        );
        let mapper = super::tests::mapper(&runtime);
        let pallet = ir
            .pallets
            .iter_mut()
            .find(|p| p.name == "SubtensorModule")
            .unwrap();
        let mut call = pallet
            .calls
            .iter()
            .find(|c| c.name == "add_stake_limit")
            .unwrap()
            .clone();
        call.name = "future_price_call".to_owned();
        pallet.calls.push(call);
        assert!(
            mapper
                .validate_prices(&ir, &scope)
                .unwrap_err()
                .to_string()
                .contains("future_price_call")
        );
    }
    #[test]
    fn selective_source_mapping_must_match_variant_order() {
        let source = "pub enum SelectiveMetagraphIndex { A, B } impl SelectiveMetagraphIndex { fn from_index(i: usize) -> Option<Self> { match i {0=>Some(SelectiveMetagraphIndex::A),1=>Some(SelectiveMetagraphIndex::B),_=>None} } }";
        assert!(selective(source).is_ok());
        assert!(
            selective(&source.replace(
                "1=>Some(SelectiveMetagraphIndex::B)",
                "1=>Some(SelectiveMetagraphIndex::A)"
            ))
            .is_err()
        );
        assert!(selective(&source.replace("A, B", "A = 4, B")).is_err());
    }
    #[test]
    fn keyword_and_acronym_names_are_stable_and_valid_rust() {
        assert_eq!(emit::ident("type"), "r#type");
        assert_eq!(emit::ident("self"), "self_");
        assert_eq!(emit::snake("SubtensorModule"), "subtensor_module");
        assert_eq!(emit::snake("EVM"), "evm");
    }

    #[test]
    fn semantic_field_contracts_require_all_overrides_and_exact_compact_wire_types() {
        let (mut runtime, _, _, _) = fixture();
        let mut missing = mapper(&runtime);
        missing.fields.remove("Metagraph.name");
        assert!(
            missing
                .model_overrides()
                .unwrap_err()
                .to_string()
                .contains("all eleven")
        );
        let type_id = mapper(&runtime)
            .model_overrides()
            .unwrap()
            .into_iter()
            .find(|field| field.model == "Metagraph" && field.field == "name")
            .unwrap()
            .type_id;
        let sequence = runtime.resolve(type_id).unwrap();
        let scale_info::TypeDef::Sequence(sequence) = &sequence.type_def else {
            unreachable!()
        };
        let scale_info::TypeDef::Compact(element) =
            &runtime.resolve(sequence.type_param.id).unwrap().type_def
        else {
            unreachable!()
        };
        let ordinary_byte = element.type_param;
        let sequence = &mut runtime
            .types
            .types
            .iter_mut()
            .find(|ty| ty.id == type_id)
            .unwrap()
            .ty
            .type_def;
        let scale_info::TypeDef::Sequence(sequence) = sequence else {
            unreachable!()
        };
        sequence.type_param = ordinary_byte;
        assert!(
            mapper(&runtime)
                .model_overrides()
                .unwrap_err()
                .to_string()
                .contains("not compact")
        );
    }

    #[test]
    fn mapped_api_models_use_declared_projections_and_detect_retyped_fields() {
        let (mut runtime, mut ir, outer, scope) = fixture();
        let root = root().unwrap();
        let config:DocumentMut="[mappings]\nMetagraph = 'crate::models::MetagraphWire'\n[projections]\nMetagraph = ['name','netuid','tempo']\n".parse().unwrap();
        fn configured<'a>(
            runtime: &'a Runtime,
            root: &Path,
            config: &DocumentMut,
        ) -> types::TypeMapper<'a> {
            types::TypeMapper::new(
                runtime,
                &read_document(root, "codegen/types.toml").unwrap(),
                config,
                &read_document(root, "codegen/overrides.toml").unwrap(),
            )
            .unwrap()
        }
        let before = emit::generate(
            &runtime,
            &ir,
            &configured(&runtime, &root, &config),
            &scope,
            outer,
            &BTreeMap::new(),
        )
        .unwrap();
        let mut invalid = configured(&runtime, &root, &config);
        invalid.projections.insert(
            "Metagraph".to_owned(),
            vec!["missing_declared_field".to_owned()],
        );
        assert!(
            invalid
                .configured_model_projections()
                .unwrap_err()
                .to_string()
                .contains("missing_declared_field")
        );
        let ty = runtime
            .types
            .types
            .iter_mut()
            .find(|ty| {
                ty.ty
                    .path
                    .segments
                    .last()
                    .is_some_and(|path| path == "Metagraph")
            })
            .unwrap();
        let scale_info::TypeDef::Composite(composite) = &mut ty.ty.type_def else {
            unreachable!()
        };
        let mut extra = composite.fields[0].clone();
        extra.name = Some("future_unused_field".to_owned());
        composite.fields.push(extra);
        let mut next = rtao_core::ir::MetadataIr::from_runtime(&runtime).unwrap();
        next.set_api_versions(
            &ir.runtime_apis
                .iter()
                .map(|api| (api.name.clone(), api.version.unwrap()))
                .collect(),
        )
        .unwrap();
        ir = next;
        let appended = emit::generate(
            &runtime,
            &ir,
            &configured(&runtime, &root, &config),
            &scope,
            outer,
            &BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(before.get("runtime_apis"), appended.get("runtime_apis"));
        assert_ne!(before.get("shapes"), appended.get("shapes"));
        assert!(
            before
                .get("runtime_apis")
                .unwrap()
                .contains("output_projection: Some(output_projection_get_metagraph)")
        );
        let boolean = runtime
            .types
            .types
            .iter()
            .find(|ty| {
                matches!(
                    ty.ty.type_def,
                    scale_info::TypeDef::Primitive(scale_info::TypeDefPrimitive::Bool)
                )
            })
            .unwrap()
            .id;
        let ty = runtime
            .types
            .types
            .iter_mut()
            .find(|ty| {
                ty.ty
                    .path
                    .segments
                    .last()
                    .is_some_and(|path| path == "Metagraph")
            })
            .unwrap();
        let scale_info::TypeDef::Composite(composite) = &mut ty.ty.type_def else {
            unreachable!()
        };
        composite
            .fields
            .iter_mut()
            .find(|field| field.name.as_deref() == Some("tempo"))
            .unwrap()
            .ty = boolean.into();
        let changed = emit::generate(
            &runtime,
            &ir,
            &configured(&runtime, &root, &config),
            &scope,
            outer,
            &BTreeMap::new(),
        )
        .unwrap();
        assert_ne!(before.get("runtime_apis"), changed.get("runtime_apis"));
    }
}
