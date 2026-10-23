//! Check handwritten model field declarations and frozen upstream layouts.
use super::types::TypeMapper;
use anyhow::{Context, Result, bail, ensure};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};
use syn::{Fields, Item};

#[derive(Debug, PartialEq, Eq)]
enum VariantFields {
    Unit,
    Named(BTreeMap<String, String>),
    Unnamed(Vec<String>),
}
#[derive(Debug)]
struct EnumDeclaration {
    variants: Vec<(String, VariantFields)>,
}
fn enum_declarations(
    root: &Path,
    configured: &BTreeSet<String>,
) -> Result<BTreeMap<String, EnumDeclaration>> {
    let mut result = BTreeMap::new();
    for path in rust_files(&root.join("crates/rtao-runtime/src/models"))? {
        let source = fs::read_to_string(&path)?;
        ensure!(
            source.len() <= 8 * 1024 * 1024,
            "model source exceeds bound"
        );
        let file =
            syn::parse_file(&source).with_context(|| format!("parse model {}", path.display()))?;
        for item in file.items {
            let Item::Enum(item) = item else { continue };
            if !configured.contains(&item.ident.to_string()) {
                continue;
            }
            let variants = item
                .variants
                .into_iter()
                .map(|variant| {
                    ensure!(
                        variant.discriminant.is_none(),
                        "hand enum discriminants need an explicit contract"
                    );
                    let fields = match variant.fields {
                        Fields::Unit => VariantFields::Unit,
                        Fields::Named(fields) => VariantFields::Named(
                            fields
                                .named
                                .iter()
                                .map(|field| {
                                    Ok((
                                        field
                                            .ident
                                            .as_ref()
                                            .context("enum field name missing")?
                                            .to_string(),
                                        normalize_type(&field.ty)?,
                                    ))
                                })
                                .collect::<Result<_>>()?,
                        ),
                        Fields::Unnamed(fields) => VariantFields::Unnamed(
                            fields
                                .unnamed
                                .iter()
                                .map(|field| normalize_type(&field.ty))
                                .collect::<Result<_>>()?,
                        ),
                    };
                    Ok((variant.ident.to_string(), fields))
                })
                .collect::<Result<Vec<_>>>()?;
            ensure!(
                result
                    .insert(item.ident.to_string(), EnumDeclaration { variants })
                    .is_none(),
                "duplicate handwritten enum {}",
                item.ident
            );
        }
    }
    Ok(result)
}

/// Named H256 is a checked `[u8;32]` wire newtype in crowdloan payloads.
fn enum_field_type(mapper: &TypeMapper<'_>, id: u32) -> Result<String> {
    let ty = mapper.runtime.resolve(id)?;
    if ty.path.segments.last().is_some_and(|name| name == "H256") {
        let scale_info::TypeDef::Composite(composite) = &ty.type_def else {
            bail!("H256 is not its array wrapper")
        };
        ensure!(
            composite.fields.len() == 1 && composite.fields[0].name.is_none(),
            "H256 wire wrapper changed"
        );
        let scale_info::TypeDef::Array(array) =
            &mapper.runtime.resolve(composite.fields[0].ty.id)?.type_def
        else {
            bail!("H256 payload is not an array")
        };
        ensure!(
            array.len == 32
                && matches!(
                    mapper.runtime.resolve(array.type_param.id)?.type_def,
                    scale_info::TypeDef::Primitive(scale_info::TypeDefPrimitive::U8)
                ),
            "H256 payload must be [u8;32]"
        );
        return Ok("[u8;32]".into());
    }
    normalize_type(&syn::parse_str::<syn::Type>(&mapper.map(id)?)?)
}
fn check_enums(
    root: &Path,
    mapper: &TypeMapper<'_>,
    models: &[super::types::ModelProjection],
) -> Result<()> {
    let configured = mapper
        .enums
        .values()
        .map(|contract| {
            contract
                .rust_type
                .rsplit("::")
                .next()
                .context("enum Rust type missing")
                .map(str::to_owned)
        })
        .collect::<Result<BTreeSet<_>>>()?;
    let actual = enum_declarations(root, &configured)?;
    for (identity, contract) in &mapper.enums {
        let rust_name = contract
            .rust_type
            .rsplit("::")
            .next()
            .context("enum Rust type missing")?;
        let declaration = actual.get(rust_name).with_context(|| {
            format!("configured enum {identity} Rust declaration {rust_name} missing")
        })?;
        ensure!(
            declaration
                .variants
                .iter()
                .map(|(name, _)| name)
                .eq(contract.variants.iter()),
            "enum {rust_name} Rust variants differ from declared contract"
        );
        let model = models
            .iter()
            .find(|model| {
                model.path == *identity || model.path.rsplit("::").next() == Some(identity.as_str())
            })
            .context("metadata enum identity missing")?;
        let scale_info::TypeDef::Variant(metadata) =
            &mapper.runtime.resolve(model.type_id)?.type_def
        else {
            bail!("metadata enum is not a variant type")
        };
        ensure!(
            metadata
                .variants
                .iter()
                .map(|variant| &variant.name)
                .collect::<BTreeSet<_>>()
                .len()
                == metadata.variants.len(),
            "enum {identity} has duplicate metadata variant names"
        );
        let mut expected = Vec::new();
        for (name, fields) in &declaration.variants {
            if contract.commitment_raw && name == "Raw" {
                ensure!(
                    *fields == VariantFields::Unnamed(vec!["Vec<u8>".into()]),
                    "Data.Raw must preserve exact raw bytes"
                );
                for len in 0..=128u32 {
                    expected.push((format!("Raw{len}"), Some(len), fields));
                }
            } else {
                expected.push((name.clone(), None, fields));
            }
        }
        ensure!(
            metadata.variants.len() == expected.len(),
            "enum {identity} metadata variant count changed"
        );
        for (index, (name, raw_length, actual)) in expected.into_iter().enumerate() {
            let variant = metadata
                .variants
                .iter()
                .find(|variant| variant.name == name)
                .with_context(|| format!("enum {identity} variant {name} missing"))?;
            ensure!(
                usize::from(variant.index) == index,
                "enum {identity}.{name} SCALE index changed"
            );
            if let Some(length) = raw_length {
                ensure!(
                    variant.fields.len() == 1 && variant.fields[0].name.is_none(),
                    "Data.{name} must have one unnamed byte array"
                );
                let scale_info::TypeDef::Array(array) =
                    &mapper.runtime.resolve(variant.fields[0].ty.id)?.type_def
                else {
                    bail!("Data.{name} must be a fixed byte array")
                };
                ensure!(
                    array.len == length
                        && matches!(
                            mapper.runtime.resolve(array.type_param.id)?.type_def,
                            scale_info::TypeDef::Primitive(scale_info::TypeDefPrimitive::U8)
                        ),
                    "Data.{name} raw family length/element changed"
                );
                continue;
            }
            let expected = if variant.fields.is_empty() {
                VariantFields::Unit
            } else if variant.fields.iter().all(|field| field.name.is_some()) {
                VariantFields::Named(
                    variant
                        .fields
                        .iter()
                        .map(|field| {
                            Ok((
                                field.name.clone().context("enum field name missing")?,
                                enum_field_type(mapper, field.ty.id)?,
                            ))
                        })
                        .collect::<Result<_>>()?,
                )
            } else {
                ensure!(
                    variant.fields.iter().all(|field| field.name.is_none()),
                    "mixed enum payload field names"
                );
                VariantFields::Unnamed(
                    variant
                        .fields
                        .iter()
                        .map(|field| enum_field_type(mapper, field.ty.id))
                        .collect::<Result<_>>()?,
                )
            };
            ensure!(
                *actual == expected,
                "enum {rust_name}.{name} Rust payload {actual:?} differs from metadata {expected:?}"
            );
        }
    }
    Ok(())
}

fn rust_files(directory: &Path) -> Result<Vec<PathBuf>> {
    let mut result = Vec::new();
    if !directory.exists() {
        return Ok(result);
    }
    ensure!(
        fs::symlink_metadata(directory)?.file_type().is_dir(),
        "model source directory must be regular"
    );
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let ty = entry.file_type()?;
        ensure!(!ty.is_symlink(), "model source symlink refused");
        if ty.is_dir() {
            result.extend(rust_files(&path)?);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            result.push(path);
        }
    }
    result.sort();
    Ok(result)
}
fn normalize_type(ty: &syn::Type) -> Result<String> {
    Ok(match ty {
        syn::Type::Path(path) => {
            let last = path.path.segments.last().context("model type path empty")?;
            let suffix = match &last.arguments {
                syn::PathArguments::None => String::new(),
                syn::PathArguments::AngleBracketed(arguments) => format!(
                    "<{}>",
                    arguments
                        .args
                        .iter()
                        .map(|argument| match argument {
                            syn::GenericArgument::Type(ty) => normalize_type(ty),
                            _ => bail!("unsupported model generic argument"),
                        })
                        .collect::<Result<Vec<_>>>()?
                        .join(",")
                ),
                _ => bail!("unsupported model type arguments"),
            };
            format!("{}{suffix}", last.ident)
        }
        syn::Type::Tuple(tuple) => format!(
            "({})",
            tuple
                .elems
                .iter()
                .map(normalize_type)
                .collect::<Result<Vec<_>>>()?
                .join(",")
        ),
        syn::Type::Array(array) => format!("[{};{}]", normalize_type(&array.elem)?, {
            let length = &array.len;
            quote::quote!(#length).to_string().replace(' ', "")
        }),
        syn::Type::Paren(paren) => normalize_type(&paren.elem)?,
        _ => bail!("unsupported handwritten model field type"),
    })
}
fn model_fields(
    root: &Path,
    configured: &BTreeSet<String>,
) -> Result<BTreeMap<String, BTreeMap<String, String>>> {
    let mut result = BTreeMap::new();
    for path in rust_files(&root.join("crates/rtao-runtime/src/models"))? {
        let source = fs::read_to_string(&path)?;
        ensure!(
            source.len() <= 8 * 1024 * 1024,
            "model source exceeds bound"
        );
        let file =
            syn::parse_file(&source).with_context(|| format!("parse model {}", path.display()))?;
        for item in file.items {
            let Item::Struct(item) = item else {
                continue;
            };
            // Test generators and private decoder helpers are not wire models.
            // Every configured declaration remains required and checked below.
            if !configured.contains(&item.ident.to_string()) {
                continue;
            }
            let Fields::Named(fields) = item.fields else {
                continue;
            };
            let names = fields
                .named
                .iter()
                .map(|field| {
                    let name = field
                        .ident
                        .as_ref()
                        .map(ToString::to_string)
                        .context("model field unnamed")?;
                    Ok((name, normalize_type(&field.ty)?))
                })
                .collect::<Result<BTreeMap<_, _>>>()?;
            ensure!(
                result.insert(item.ident.to_string(), names).is_none(),
                "duplicate handwritten model type {}",
                item.ident
            );
        }
    }
    Ok(result)
}
pub(super) fn check(root: &Path, mapper: &TypeMapper<'_>) -> Result<()> {
    let _ = mapper.model_overrides()?;
    let configured = mapper
        .models
        .values()
        .map(|path| {
            path.rsplit("::")
                .next()
                .map(str::to_owned)
                .context("model Rust type missing")
        })
        .collect::<Result<BTreeSet<_>>>()?;
    let fields = model_fields(root, &configured)?;
    let models = mapper.configured_model_projections()?;
    for (metadata, rust_type) in &mapper.models {
        let rust_name = rust_type
            .rsplit("::")
            .next()
            .context("model Rust type missing")?;
        let actual = fields.get(rust_name).with_context(|| {
            format!("configured model {metadata} Rust declaration {rust_name} missing")
        })?;
        let declared = mapper
            .projections
            .get(metadata)
            .context("model projection missing")?
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        ensure!(
            actual.keys().cloned().collect::<BTreeSet<_>>() == declared,
            "model {rust_name} Rust fields differ from configured projection; update declared fields deliberately"
        );
        let model = models
            .iter()
            .find(|model| {
                model.path == *metadata || model.path.rsplit("::").next() == Some(metadata.as_str())
            })
            .context("metadata model identity missing")?;
        let scale_info::TypeDef::Composite(composite) =
            &mapper.runtime.resolve(model.type_id)?.type_def
        else {
            bail!("metadata model is not composite")
        };
        let ident = metadata
            .rsplit("::")
            .next()
            .context("model identity empty")?;
        for (name, actual) in actual {
            let field = composite
                .fields
                .iter()
                .find(|field| field.name.as_deref() == Some(name))
                .context("declared field missing from metadata")?;
            let expected = mapper
                .fields
                .get(&format!("{ident}.{name}"))
                .cloned()
                .map(Ok)
                .unwrap_or_else(|| mapper.map(field.ty.id))?;
            let expected = normalize_type(&syn::parse_str::<syn::Type>(&expected)?)?;
            ensure!(
                actual == &expected,
                "model {rust_name}.{name} Rust type {actual} differs from metadata mapping {expected}"
            );
        }
    }
    check_enums(root, mapper, &models)?;
    // Ensure registry field names/types and recursive projections are all resolvable.
    let _ = mapper.configured_model_projections()?;
    Ok(())
}

fn frozen_source(root: &Path) -> Result<BTreeMap<String, String>> {
    let mut result = BTreeMap::new();
    for path in rust_files(&root.join("vendor/upstream/src/pallets/subtensor/src/rpc_info"))? {
        let source = fs::read_to_string(&path)?;
        let file = syn::parse_file(&source)
            .with_context(|| format!("parse frozen source {}", path.display()))?;
        for item in file.items {
            let Item::Struct(item) = item else {
                continue;
            };
            if let Some(attribute) = item
                .attrs
                .iter()
                .find(|attribute| attribute.path().is_ident("freeze_struct"))
            {
                let value = attribute
                    .parse_args::<syn::LitStr>()
                    .context("freeze_struct hash must be literal")?
                    .value();
                ensure!(
                    !value.is_empty()
                        && value.len() <= 16
                        && value.bytes().all(|byte| byte.is_ascii_hexdigit()),
                    "freeze_struct hash invalid"
                );
                let key = format!(
                    "{}::{}",
                    path.strip_prefix(root.join("vendor/upstream/src"))?
                        .to_string_lossy(),
                    item.ident
                );
                ensure!(
                    result.insert(key, value).is_none(),
                    "duplicate frozen struct"
                );
            }
        }
    }
    ensure!(
        !result.is_empty(),
        "frozen runtime struct inventory is empty"
    );
    Ok(result)
}
pub(super) fn freeze_tripwires(root: &Path, check: bool) -> Result<()> {
    let expected = frozen_source(root)?;
    let path = root.join("codegen/frozen_models.toml");
    if !path.exists() {
        ensure!(
            !check,
            "frozen model tripwires missing; run cargo xtask regen once"
        );
        let mut output = String::from(
            "# Generated once from pinned upstream freeze_struct attributes; changes require model review.\n[hashes]\n",
        );
        for (key, value) in &expected {
            output.push_str(&format!("{key:?} = {value:?}\n"));
        }
        fs::write(&path, output)?;
    }
    ensure!(
        fs::symlink_metadata(&path)?.file_type().is_file(),
        "frozen model snapshot must be regular"
    );
    let document = fs::read_to_string(path)?.parse::<toml_edit::DocumentMut>()?;
    let actual = document
        .get("hashes")
        .and_then(|item| item.as_table_like())
        .context("frozen [hashes] missing")?
        .iter()
        .map(|(key, value)| {
            Ok((
                key.to_owned(),
                value
                    .as_str()
                    .context("frozen hash must be string")?
                    .to_owned(),
            ))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;
    ensure!(
        actual == expected,
        "pinned freeze_struct layout hashes changed; review models and update codegen/frozen_models.toml deliberately"
    );
    Ok(())
}
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    fn pinned_runtime() -> rtao_core::runtime::Runtime {
        rtao_core::runtime::Runtime::parse(
            include_bytes!("../../vendor/metadata/finney/482.v15.scale"),
            482,
            1,
            42,
        )
        .unwrap()
    }
    fn pinned_mapper(runtime: &rtao_core::runtime::Runtime) -> TypeMapper<'_> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let document = |name| {
            fs::read_to_string(root.join(name))
                .unwrap()
                .parse::<toml_edit::DocumentMut>()
                .unwrap()
        };
        TypeMapper::new(
            runtime,
            &document("codegen/types.toml"),
            &document("codegen/models.toml"),
            &document("codegen/overrides.toml"),
        )
        .unwrap()
    }
    #[test]
    fn complete_hand_models_and_exact_enums_match_pinned_metadata() {
        let runtime = pinned_runtime();
        let mapper = pinned_mapper(&runtime);
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        check(root, &mapper).unwrap();
        assert_eq!(mapper.models.len(), 31);
        assert_eq!(mapper.enums.len(), 4);
    }
    #[test]
    fn exact_enum_contracts_refuse_new_variants_retyping_and_raw_family_drift() {
        use scale_info::{TypeDef, TypeDefPrimitive};
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        for mutation in 0..4 {
            let mut runtime = pinned_runtime();
            let boolean = runtime
                .types
                .types
                .iter()
                .find(|ty| matches!(ty.ty.type_def, TypeDef::Primitive(TypeDefPrimitive::Bool)))
                .unwrap()
                .id;
            let enum_name = if mutation == 0 {
                "ProxyType"
            } else if mutation == 1 {
                "HyperparamValue"
            } else {
                "Data"
            };
            let ty = runtime
                .types
                .types
                .iter_mut()
                .find(|ty| {
                    ty.ty
                        .path
                        .segments
                        .last()
                        .is_some_and(|name| name == enum_name)
                })
                .unwrap();
            let TypeDef::Variant(variants) = &mut ty.ty.type_def else {
                unreachable!()
            };
            match mutation {
                0 => {
                    let mut added = variants.variants[0].clone();
                    added.name = "NewPermission".into();
                    added.index = 200;
                    variants.variants.push(added);
                }
                1 => {
                    variants
                        .variants
                        .iter_mut()
                        .find(|variant| variant.name == "U64")
                        .unwrap()
                        .fields[0]
                        .ty = boolean.into();
                }
                2 => {
                    variants
                        .variants
                        .iter_mut()
                        .find(|variant| variant.name == "Raw128")
                        .unwrap()
                        .index = 250;
                }
                _ => {
                    variants
                        .variants
                        .iter_mut()
                        .find(|variant| variant.name == "Raw128")
                        .unwrap()
                        .fields[0]
                        .ty = boolean.into();
                }
            }
            let mapper = pinned_mapper(&runtime);
            let models = mapper.configured_model_projections().unwrap();
            assert!(
                check_enums(root, &mapper, &models).is_err(),
                "mutation {mutation} passed"
            );
        }
    }
    #[test]
    fn pinned_frozen_tripwire_inventory_includes_both_metagraphs() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let frozen = frozen_source(root).unwrap();
        assert_eq!(
            frozen
                .get("pallets/subtensor/src/rpc_info/metagraph.rs::Metagraph")
                .map(String::as_str),
            Some("93460b9b3cbf6d4e")
        );
        assert_eq!(
            frozen
                .get("pallets/subtensor/src/rpc_info/metagraph.rs::SelectiveMetagraph")
                .map(String::as_str),
            Some("bb7420226d39c0eb")
        );
        assert!(frozen.len() > 20);
    }
    #[test]
    fn qualified_rust_field_types_match_imported_names_but_retypes_do_not() {
        let normalize =
            |value| normalize_type(&syn::parse_str::<syn::Type>(value).unwrap()).unwrap();
        assert_eq!(
            normalize("Vec<(rtao_primitives::AccountId, Option<crate::models::AxonInfo>)>"),
            normalize("Vec<(AccountId,Option<AxonInfo>,)>")
        );
        assert_ne!(normalize("u64"), normalize("u32"));
        assert_eq!(normalize("[u8; 32]"), "[u8;32]");
        assert_ne!(normalize("TaoBalance"), normalize("AlphaBalance"));
    }
    #[test]
    fn borrowed_test_helpers_do_not_enter_configured_wire_models() -> Result<()> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .context("workspace root missing")?;
        let configured = BTreeSet::from(["MetagraphWire".to_owned()]);
        let declarations = model_fields(root, &configured)?;
        ensure!(declarations.keys().cloned().collect::<BTreeSet<_>>() == configured);
        ensure!(
            declarations["MetagraphWire"]
                .get("hotkeys")
                .map(String::as_str)
                == Some("Vec<AccountId>")
        );
        // A borrowed wire field still cannot satisfy an owned metadata mapping.
        ensure!(normalize_type(&syn::parse_str::<syn::Type>("&AccountId")?).is_err());
        Ok(())
    }
}
