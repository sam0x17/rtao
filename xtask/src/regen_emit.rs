//! Deterministic descriptor and typed-builder emission.

use super::shape::{OuterEnums, ShapeHash, ShapeHasher};
use super::shape_parts;
use super::types::{TypeMapper, named_fields};
use anyhow::{Context, Result, ensure};
use rtao_core::{
    ir::{FieldIr, MetadataIr},
    runtime::Runtime,
};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) fn snake(name: &str) -> String {
    let chars: Vec<_> = name.chars().collect();
    let mut out = String::new();
    for (position, ch) in chars.iter().copied().enumerate() {
        if ch.is_uppercase()
            && position > 0
            && chars
                .get(position - 1)
                .is_some_and(|c| c.is_lowercase() || c.is_numeric())
        {
            out.push('_');
        }
        out.extend(ch.to_lowercase());
    }
    out
}
pub(crate) fn ident(name: &str) -> String {
    match name {
        "self" | "Self" | "super" | "crate" => format!("{name}_"),
        "as" | "async" | "await" | "break" | "const" | "continue" | "dyn" | "else" | "enum"
        | "extern" | "false" | "fn" | "for" | "if" | "impl" | "in" | "let" | "loop" | "match"
        | "mod" | "move" | "mut" | "pub" | "ref" | "return" | "static" | "struct" | "trait"
        | "true" | "type" | "unsafe" | "use" | "where" | "while" | "yield" | "try" | "gen" => {
            format!("r#{name}")
        }
        _ => name.to_owned(),
    }
}
fn hash(shape: ShapeHash) -> String {
    format!("crate::ShapeHash({:?})", shape.0)
}
fn fields(fields: &[FieldIr]) -> String {
    format!(
        "&[{}]",
        fields
            .iter()
            .map(|field| format!(
                "crate::FieldDescriptor {{ name: {:?}, type_ident: {:?} }}",
                field.name, field.type_ident
            ))
            .collect::<Vec<_>>()
            .join(",")
    )
}
fn tuple(types: &[String]) -> String {
    format!(
        "({})",
        types
            .iter()
            .map(|s| format!("{s},"))
            .collect::<Vec<_>>()
            .join(" ")
    )
}
fn module_start(out: &mut String, pallet: &str) {
    out.push_str(&format!(
        "/// Descriptors for the `{pallet}` pallet.\npub mod {} {{\n",
        snake(pallet)
    ));
}
fn unique(names: impl IntoIterator<Item = String>, kind: &str) -> Result<()> {
    let mut seen = BTreeSet::new();
    for name in names {
        ensure!(
            seen.insert(name.clone()),
            "{kind} emitted name collision {name}"
        );
    }
    Ok(())
}

pub(crate) fn generate(
    runtime: &Runtime,
    ir: &MetadataIr,
    mapper: &TypeMapper<'_>,
    scope: &BTreeSet<String>,
    outer: OuterEnums,
    deprecations: &BTreeMap<(String, u8), String>,
) -> Result<BTreeMap<String, String>> {
    let hasher = ShapeHasher::new(&runtime.types, outer);
    let mut bodies: BTreeMap<String, String> = [
        "storage",
        "calls",
        "runtime_apis",
        "constants",
        "events",
        "errors",
        "extensions",
        "selective",
        "shapes",
    ]
    .into_iter()
    .map(|name| (name.to_owned(), String::new()))
    .collect();
    let mut call_all = Vec::new();
    let mut storage_all = Vec::new();
    let mut constant_all = Vec::new();
    let mut event_all = Vec::new();
    let mut api_all = Vec::new();
    let mut used = Vec::new();
    let mut pallets: Vec<_> = ir.pallets.iter().collect();
    pallets.sort_by(|a, b| a.name.cmp(&b.name));
    unique(pallets.iter().map(|p| snake(&p.name)), "pallet")?;
    for pallet in &pallets {
        if !scope.contains(&pallet.name) {
            continue;
        }
        let module = snake(&pallet.name);
        for name in ["storage", "calls", "constants", "events"] {
            module_start(bodies.get_mut(name).context("body missing")?, &pallet.name);
        }
        let mut calls: Vec<_> = pallet.calls.iter().collect();
        calls.sort_by(|a, b| a.name.cmp(&b.name));
        unique(calls.iter().map(|c| ident(&c.name)), "call")?;
        for call in calls {
            let const_name = snake(&call.name).to_uppercase();
            let shape =
                hasher.exact_fields(named_fields(runtime, &pallet.name, &call.name, false)?)?;
            let out = bodies.get_mut("calls").context("calls body missing")?;
            out.push_str(&format!("/// `{}` call argument contract.\npub const {const_name}: crate::CallDescriptor = crate::CallDescriptor {{ pallet: {:?}, name: {:?}, fields: {}, shape: {}, pallet_index: {}, call_index: {} }};\n",call.name,pallet.name,call.name,fields(&call.args),hash(shape),pallet.index,call.index));
            let arguments = call
                .args
                .iter()
                .map(|arg| {
                    Ok((
                        ident(&arg.name),
                        mapper.argument(&pallet.name, &call.name, arg)?,
                    ))
                })
                .collect::<Result<Vec<_>>>()?;
            unique(arguments.iter().map(|(name, _)| name.clone()), "argument")?;
            if let Some(note) = deprecations.get(&(pallet.name.clone(), call.index)) {
                out.push_str(&format!("#[deprecated(note = {note:?})]\n"));
            }
            out.push_str(&format!("/// Build `{}::{}` without encoding or doing I/O.\npub fn {}({}) -> crate::Call {{\n",pallet.name,call.name,ident(&call.name),arguments.iter().map(|(name,rust)|format!("{name}: {rust}")).collect::<Vec<_>>().join(",")));
            let nested: Vec<_> = arguments
                .iter()
                .filter(|(_, rust)| rust.contains("crate::CallBytes"))
                .collect();
            if nested.is_empty() {
                out.push_str("let nested_stamps = Vec::new();\n");
            } else {
                out.push_str("let mut nested_stamps = Vec::new();\n");
                for (name, _) in nested {
                    out.push_str(&format!(
                        "nested_stamps.extend(crate::nested_call_stamps(&{name}));\n"
                    ));
                }
            }
            out.push_str(&format!("crate::Call::new(&{const_name}, rtao_core::codec::Value::record(vec![{}]), nested_stamps)\n}}\n",call.args.iter().zip(&arguments).map(|(field,(name,_))|format!("({:?}.to_owned(),rtao_primitives::IntoCoreValue::into_core_value({name}))",field.name)).collect::<Vec<_>>().join(",")));
            let path = format!("{module}::{const_name}");
            call_all.push(format!("&{path}"));
            used.push(format!(
                "crate::SurfaceDescriptor::Call(&super::calls::{path})"
            ));
        }
        let mut storage: Vec<_> = pallet.storage.iter().collect();
        storage.sort_by(|a, b| a.name.cmp(&b.name));
        unique(
            storage.iter().map(|s| snake(&s.name).to_uppercase()),
            "storage",
        )?;
        for entry in storage {
            let const_name = snake(&entry.name).to_uppercase();
            let mut ids = entry.key_types.clone();
            ids.push(entry.value_type);
            let shape = hasher.exact_tuple(&ids)?;
            let keys = entry
                .key_types
                .iter()
                .map(|id| mapper.map(*id))
                .collect::<Result<Vec<_>>>()?;
            let value = if entry.modifier == "Default"
                && entry.value_type_ident == "AccountId32"
                && entry.default_bytes == [0; 32]
            {
                "rtao_primitives::DefaultAccount".to_owned()
            } else {
                mapper.map(entry.value_type)?
            };
            let value = if entry.modifier == "Optional" {
                format!("crate::OptionalStorage<{value}>")
            } else {
                value
            };
            let surface = format!(
                "crate::StorageSurface {{ pallet: {:?}, name: {:?}, prefix: {:?}, key_type_idents: &{:?}, value_type_ident: {:?}, hashers: &{:?}, modifier: {:?}, default_bytes: &{:?}, shape: {} }}",
                pallet.name,
                entry.name,
                entry.prefix,
                entry.key_type_idents,
                entry.value_type_ident,
                entry.hashers,
                entry.modifier,
                entry.default_bytes,
                hash(shape)
            );
            bodies.get_mut("storage").context("storage body missing")?.push_str(&format!("/// `{}.{}` resolved through live storage metadata.\npub const {const_name}: crate::StorageDescriptor<{},{}> = crate::StorageDescriptor::new({surface});\n",pallet.name,entry.name,tuple(&keys),value));
            let path = format!("{module}::{const_name}.surface()");
            storage_all.push(path.clone());
            used.push(format!(
                "crate::SurfaceDescriptor::Storage(super::storage::{path})"
            ));
        }
        let mut constants: Vec<_> = pallet.constant_details.iter().collect();
        constants.sort_by(|a, b| a.name.cmp(&b.name));
        unique(
            constants.iter().map(|c| snake(&c.name).to_uppercase()),
            "constant",
        )?;
        for constant in constants {
            let is_version = pallet.name == "System" && constant.name == "Version";
            let name = snake(&constant.name).to_uppercase();
            let rust = mapper.map(constant.type_id)?;
            let surface = format!(
                "crate::ConstantSurface {{ pallet: {:?}, name: {:?}, type_ident: {:?}, value: &{:?}, shape: {} }}",
                pallet.name,
                constant.name,
                constant.type_ident,
                if is_version {
                    &[][..]
                } else {
                    &constant.value[..]
                },
                hash(hasher.exact(constant.type_id)?)
            );
            bodies.get_mut("constants").context("constant body missing")?.push_str(&format!("/// `{}` metadata constant.\npub const {name}: crate::ConstantDescriptor<{rust}> = crate::ConstantDescriptor::new({surface});\n",constant.name));
            let path = format!("{module}::{name}.surface()");
            constant_all.push(path.clone());
            if !is_version {
                used.push(format!(
                    "crate::SurfaceDescriptor::Constant(super::constants::{path})"
                ));
            }
        }
        let mut events: Vec<_> = pallet.events.iter().collect();
        events.sort_by(|a, b| a.name.cmp(&b.name));
        unique(
            events.iter().map(|e| snake(&e.name).to_uppercase()),
            "event",
        )?;
        for event in events {
            let name = snake(&event.name).to_uppercase();
            let shape =
                hasher.exact_fields(named_fields(runtime, &pallet.name, &event.name, true)?)?;
            bodies.get_mut("events").context("events body missing")?.push_str(&format!("/// `{}` indexed event.\npub const {name}: crate::EventDescriptor = crate::EventDescriptor {{ pallet: {:?}, name: {:?}, fields: {}, shape: {}, pallet_index: {}, event_index: {} }};\n",event.name,pallet.name,event.name,fields(&event.fields),hash(shape),pallet.index,event.index));
            let path = format!("{module}::{name}");
            event_all.push(format!("&{path}"));
            used.push(format!(
                "crate::SurfaceDescriptor::Event(&super::events::{path})"
            ));
        }
        for name in ["storage", "calls", "constants", "events"] {
            bodies
                .get_mut(name)
                .context("body missing")?
                .push_str("}\n");
        }
    }
    for (name, values, ty) in [
        ("calls", call_all, "&crate::CallDescriptor"),
        ("storage", storage_all, "crate::StorageSurface"),
        ("constants", constant_all, "crate::ConstantSurface"),
        ("events", event_all, "&crate::EventDescriptor"),
    ] {
        bodies
            .get_mut(name)
            .context("body missing")?
            .push_str(&format!(
                "/// Complete generated {name} inventory.\npub const ALL: &[{ty}] = &[{}];\n",
                values.join(",")
            ));
    }
    let mut apis: Vec<_> = ir.runtime_apis.iter().collect();
    apis.sort_by(|a, b| a.name.cmp(&b.name));
    unique(apis.iter().map(|a| snake(&a.name)), "API")?;
    for api in apis {
        let module = snake(&api.name);
        let out = bodies.get_mut("runtime_apis").context("API body missing")?;
        module_start(out, &api.name);
        let mut methods: Vec<_> = api.method_signatures.iter().collect();
        methods.sort_by(|a, b| a.name.cmp(&b.name));
        for method in methods {
            let name = snake(&method.name).to_uppercase();
            let inputs = method
                .inputs
                .iter()
                .map(|input| mapper.map(input.type_id))
                .collect::<Result<Vec<_>>>()?;
            let output = mapper.map(method.output)?;
            let mut parts = method
                .inputs
                .iter()
                .map(|input| Ok((input.name.as_str(), hasher.exact(input.type_id)?)))
                .collect::<Result<Vec<_>>>()?;
            let projected = mapper.output_projection(method.output)?;
            let output_projection = if let Some(projection) = &projected {
                let function = format!("output_projection_{}", snake(&method.name));
                out.push_str(&format!(
                    "fn {function}() -> crate::Projection {{ {} }}\n",
                    projection_source(projection)
                ));
                format!("Some({function})")
            } else {
                "None".to_owned()
            };
            let output_shape = match projected {
                Some(projection) => hasher.projection(method.output, &projection)?,
                None => hasher.exact(method.output)?,
            };
            parts.push(("$output", output_shape));
            let surface = format!(
                "crate::ApiSurface {{ api: {:?}, name: {:?}, inputs: {}, output_type_ident: {:?}, version: {}, shape: {}, output_projection: {output_projection} }}",
                api.name,
                method.name,
                fields(&method.inputs),
                method.output_type_ident,
                api.version.context("API version unresolved")?,
                hash(shape_parts(&parts))
            );
            out.push_str(&format!("/// `{}::{}` runtime API signature.\npub const {name}: crate::RuntimeApiDescriptor<{},{}> = crate::RuntimeApiDescriptor::new({surface});\n",api.name,method.name,tuple(&inputs),output));
            let path = format!("{module}::{name}.surface()");
            api_all.push(path.clone());
            used.push(format!(
                "crate::SurfaceDescriptor::RuntimeApi(super::runtime_apis::{path})"
            ));
        }
        out.push_str("}\n");
    }
    bodies
        .get_mut("runtime_apis")
        .context("API body missing")?
        .push_str(&format!(
            "/// Complete runtime API inventory.\npub const ALL: &[crate::ApiSurface] = &[{}];\n",
            api_all.join(",")
        ));
    let errors = bodies.get_mut("errors").context("errors body missing")?;
    errors.push_str("/// Every module error, including pallets outside typed call scope.\npub const MODULE_ERRORS: &[crate::ModuleError] = &[\n");
    let mut by_index = pallets.clone();
    by_index.sort_by_key(|p| p.index);
    for pallet in &by_index {
        let mut list: Vec<_> = pallet.errors.iter().collect();
        list.sort_by_key(|e| e.index);
        for error in list {
            errors.push_str(&format!("crate::ModuleError {{ pallet: {:?}, pallet_index: {}, error_index: {}, name: {:?}, docs: {:?} }},\n",pallet.name,pallet.index,error.index,error.name,error.docs));
        }
    }
    errors.push_str("];\n");
    let extensions = bodies
        .get_mut("extensions")
        .context("extensions body missing")?;
    extensions.push_str("/// Signed extensions in runtime encoding order.\npub const EXPECTED: &[crate::ExtensionDescriptor] = &[\n");
    for extension in &ir.extrinsic.signed_extensions {
        extensions.push_str(&format!("crate::ExtensionDescriptor {{ identifier: {:?}, type_ident: {:?}, additional_signed_type_ident: {:?}, shape: {}, additional_signed_shape: {} }},\n",extension.identifier,extension.type_ident,extension.additional_signed_type_ident,hash(hasher.exact(extension.type_id)?),hash(hasher.exact(extension.additional_signed)?)));
    }
    extensions.push_str("];\n");
    let shapes = bodies.get_mut("shapes").context("shapes body missing")?;
    let overrides = mapper.model_overrides()?;
    let models: BTreeSet<_> = overrides.iter().map(|entry| entry.model.as_str()).collect();
    for model in models {
        shapes.push_str(&format!(
            "/// Semantic field contracts for `{model}`.\npub mod {} {{\n",
            snake(model)
        ));
        for entry in overrides.iter().filter(|entry| entry.model == model) {
            let name = snake(&entry.field).to_uppercase();
            shapes.push_str(&format!("/// Typed semantic contract of `{model}.{}`.\npub const {name}: crate::ModelFieldDescriptor<{}> = crate::ModelFieldDescriptor::new(crate::ModelFieldSurface {{ type_path: {:?}, name: {:?}, type_ident: {:?}, shape: {} }});\n", entry.field,entry.rust_type,entry.path,entry.field,runtime.type_ident(entry.type_id),hash(hasher.exact(entry.type_id)?)));
        }
        let ty = runtime
            .types
            .types
            .iter()
            .find(|ty| ty.ty.path.segments.last().is_some_and(|name| name == model))
            .context("model type missing")?;
        shapes.push_str(&format!("/// Exact positional contract for `{model}`.\npub const EXACT: crate::ShapeHash = {};\n",hash(hasher.exact(ty.id)?)));
        shapes.push_str("}\n");
    }
    shapes.push_str("/// Configured hand-model projections included in the used surface.\npub fn models() -> Vec<crate::SurfaceDescriptor> { vec![\n");
    for model in mapper.configured_model_projections()? {
        shapes.push_str(&format!("crate::SurfaceDescriptor::Model(crate::ModelSurface {{ type_path: {:?}, projection: {}, shape: {} }}),\n",model.path,projection_source(&model.projection),hash(hasher.projection(model.type_id,&model.projection)?)));
    }
    shapes.push_str("] }\n");
    shapes.push_str(&format!("/// Generated descriptors checked before typed use.\npub const USED: &[crate::SurfaceDescriptor] = &[{}];\n",used.join(",")));
    shapes.push_str("/// Complete generated surface; the used subset narrows when read/intent declarations land.\npub const ALL: &[crate::SurfaceDescriptor] = USED;\n");
    shapes.push_str("/// Name table for all runtime pallets.\npub const PALLETS: &[crate::PalletDescriptor] = &[\n");
    for pallet in by_index {
        shapes.push_str(&format!(
            "crate::PalletDescriptor {{ name: {:?}, index: {} }},\n",
            pallet.name, pallet.index
        ));
    }
    shapes.push_str("];\n");
    Ok(bodies)
}

fn projection_source(projection: &super::shape::Projection) -> String {
    use super::shape::{Projection, ProjectionFields};
    fn fields(fields: &[super::shape::ProjectionField]) -> String {
        fields
            .iter()
            .map(|field| {
                format!(
                    "crate::ProjectionField::new({:?}, {})",
                    field.name,
                    projection_source(&field.projection)
                )
            })
            .collect::<Vec<_>>()
            .join(",")
    }
    match projection {
        Projection::Exact => "crate::Projection::Exact".to_owned(),
        Projection::Composite(selected) => {
            format!("crate::Projection::Composite(vec![{}])", fields(selected))
        }
        Projection::CompositeUnnamed(elements) => format!(
            "crate::Projection::CompositeUnnamed(vec![{}])",
            elements
                .iter()
                .map(projection_source)
                .collect::<Vec<_>>()
                .join(",")
        ),
        Projection::Variant(variants) => format!(
            "crate::Projection::Variant(vec![{}])",
            variants
                .iter()
                .map(|variant| {
                    let selected = match &variant.fields {
                        ProjectionFields::Exact => "crate::ProjectionFields::Exact".to_owned(),
                        ProjectionFields::Named(selected) => {
                            format!("crate::ProjectionFields::Named(vec![{}])", fields(selected))
                        }
                        ProjectionFields::Unnamed(selected) => format!(
                            "crate::ProjectionFields::Unnamed(vec![{}])",
                            selected
                                .iter()
                                .map(projection_source)
                                .collect::<Vec<_>>()
                                .join(",")
                        ),
                    };
                    format!(
                        "crate::ProjectionVariant {{ name: {:?}.to_owned(), fields: {selected} }}",
                        variant.name
                    )
                })
                .collect::<Vec<_>>()
                .join(",")
        ),
        Projection::Sequence(inner) => format!(
            "crate::Projection::Sequence(Box::new({}))",
            projection_source(inner)
        ),
        Projection::Array(inner) => format!(
            "crate::Projection::Array(Box::new({}))",
            projection_source(inner)
        ),
        Projection::Compact(inner) => format!(
            "crate::Projection::Compact(Box::new({}))",
            projection_source(inner)
        ),
        Projection::Tuple(elements) => format!(
            "crate::Projection::Tuple(vec![{}])",
            elements
                .iter()
                .map(projection_source)
                .collect::<Vec<_>>()
                .join(",")
        ),
    }
}
