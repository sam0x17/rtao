//! Metadata-driven Rust type mapping and fail-closed semantic overrides.

use anyhow::{Context, Result, bail, ensure};
use rtao_core::{
    ir::{FieldIr, MetadataIr},
    runtime::Runtime,
};
use scale_info::{TypeDef, form::PortableForm};
use std::collections::{BTreeMap, BTreeSet};
use toml_edit::DocumentMut;

pub(crate) struct TypeMapper<'a> {
    pub runtime: &'a Runtime,
    pub mappings: BTreeMap<String, String>,
    pub arguments: BTreeMap<(String, String, String), String>,
    pub fields: BTreeMap<String, String>,
    pub models: BTreeMap<String, String>,
    pub projections: BTreeMap<String, Vec<String>>,
    pub enums: BTreeMap<String, EnumContract>,
}
#[derive(Debug)]
pub(crate) struct EnumContract {
    pub rust_type: String,
    pub variants: Vec<String>,
    /// Commitment Data's `Raw(Vec<u8>)` represents Raw0 through Raw128.
    pub commitment_raw: bool,
}
fn parse_mappings(document: &DocumentMut) -> Result<BTreeMap<String, String>> {
    document
        .get("mappings")
        .and_then(|v| v.as_table_like())
        .context("[mappings] missing")?
        .iter()
        .map(|(key, value)| {
            Ok((
                key.to_owned(),
                value.as_str().context("mapping must be string")?.to_owned(),
            ))
        })
        .collect()
}
impl<'a> TypeMapper<'a> {
    pub fn new(
        runtime: &'a Runtime,
        types: &DocumentMut,
        models: &DocumentMut,
        overrides: &DocumentMut,
    ) -> Result<Self> {
        ensure!(
            overrides
                .get("storage")
                .and_then(|item| item.get("zero_account_default"))
                .and_then(|item| item.as_bool())
                == Some(true),
            "zero-account Default storage rule must be explicitly enabled"
        );
        let mut mappings = parse_mappings(types)?;
        let model_mappings = parse_mappings(models)?;
        let mut enums = BTreeMap::new();
        if let Some(item) = models.get("enums") {
            let rows = item
                .as_array_of_tables()
                .context("model enums must be [[enums]] rows")?;
            for row in rows {
                let metadata = row
                    .get("metadata")
                    .and_then(|item| item.as_str())
                    .context("enum metadata identity missing")?;
                let rust_type = row
                    .get("rust_type")
                    .and_then(|item| item.as_str())
                    .context("enum Rust type missing")?
                    .to_owned();
                let variants = row
                    .get("variants")
                    .and_then(|item| item.as_array())
                    .context("enum variants must be declared in SCALE-index order")?
                    .iter()
                    .map(|item| {
                        item.as_str()
                            .map(str::to_owned)
                            .context("enum variant must be string")
                    })
                    .collect::<Result<Vec<_>>>()?;
                ensure!(
                    !variants.is_empty() && variants.len() <= 256,
                    "invalid enum variant count {metadata}"
                );
                ensure!(
                    variants.iter().collect::<BTreeSet<_>>().len() == variants.len(),
                    "duplicate enum variant {metadata}"
                );
                let custom = row
                    .get("custom")
                    .map(|item| item.as_str().context("enum custom contract must be string"))
                    .transpose()?;
                let commitment_raw = match custom {
                    None => false,
                    Some("commitment-raw-0-128") => {
                        ensure!(
                            metadata.rsplit("::").next() == Some("Data")
                                && variants.iter().any(|name| name == "Raw"),
                            "commitment raw contract requires Data.Raw"
                        );
                        true
                    }
                    Some(custom) => bail!("unknown enum custom contract {custom}"),
                };
                ensure!(
                    enums
                        .insert(
                            metadata.to_owned(),
                            EnumContract {
                                rust_type,
                                variants,
                                commitment_raw
                            }
                        )
                        .is_none(),
                    "duplicate enum contract {metadata}"
                );
            }
        }
        let mut projections = BTreeMap::new();
        if let Some(table) = models
            .get("projections")
            .and_then(|item| item.as_table_like())
        {
            for (path, value) in table.iter() {
                let mut fields = value
                    .as_array()
                    .context("model projection fields must be an array")?
                    .iter()
                    .map(|item| {
                        item.as_str()
                            .map(str::to_owned)
                            .context("projection field must be string")
                    })
                    .collect::<Result<Vec<_>>>()?;
                let count = fields.len();
                fields.sort();
                fields.dedup();
                ensure!(
                    fields.len() == count,
                    "duplicate projected model field {path}"
                );
                ensure!(!fields.is_empty(), "empty model projection {path}");
                projections.insert(path.to_owned(), fields);
            }
        }
        ensure!(
            model_mappings.keys().eq(projections.keys()),
            "every hand-model mapping requires its declared projection fields"
        );
        for (name, rust) in &model_mappings {
            ensure!(
                mappings.insert(name.clone(), rust.clone()).is_none(),
                "duplicate type mapping {name}"
            );
        }
        for (name, contract) in &enums {
            ensure!(
                mappings
                    .insert(name.clone(), contract.rust_type.clone())
                    .is_none(),
                "duplicate enum type mapping {name}"
            );
        }
        let mut arguments = BTreeMap::new();
        for row in overrides
            .get("arguments")
            .and_then(|v| v.as_array_of_tables())
            .context("arguments missing")?
        {
            let value = |name| {
                row.get(name)
                    .and_then(|v| v.as_str())
                    .with_context(|| format!("argument override {name} missing"))
            };
            let key = (
                value("pallet")?.to_owned(),
                value("call")?.to_owned(),
                value("argument")?.to_owned(),
            );
            let kind = value("kind")?;
            ensure!(
                matches!(kind, "price" | "amount"),
                "unknown argument override kind {kind}"
            );
            ensure!(
                arguments.insert(key, kind.to_owned()).is_none(),
                "duplicate argument override"
            );
        }
        let fields = overrides
            .get("fields")
            .and_then(|v| v.as_table_like())
            .context("[fields] missing")?
            .iter()
            .map(|(key, value)| {
                Ok((
                    key.to_owned(),
                    value
                        .as_str()
                        .context("field mapping must be string")?
                        .to_owned(),
                ))
            })
            .collect::<Result<_>>()?;
        Ok(Self {
            runtime,
            mappings,
            arguments,
            fields,
            models: model_mappings,
            projections,
            enums,
        })
    }
    pub fn validate_prices(&self, ir: &MetadataIr, scope: &BTreeSet<String>) -> Result<()> {
        let mut seen = BTreeSet::new();
        for pallet in ir.pallets.iter().filter(|p| scope.contains(&p.name)) {
            for call in &pallet.calls {
                for argument in &call.args {
                    let key = (
                        pallet.name.clone(),
                        call.name.clone(),
                        argument.name.clone(),
                    );
                    let balance = self.balance_argument(argument.type_id)?;
                    if let Some(kind) = self.arguments.get(&key) {
                        ensure!(
                            balance
                                || (kind == "amount" && self.amount_argument(argument.type_id)?),
                            "override {key:?} must target a balance argument (got {})",
                            argument.type_ident
                        );
                        ensure!(
                            kind == "amount"
                                || argument.name.contains("price")
                                || argument.name.contains("limit"),
                            "price override has no price/limit name: {key:?}"
                        );
                        seen.insert(key.clone());
                    }
                    if balance
                        && (argument.name.contains("price") || argument.name.contains("limit"))
                    {
                        ensure!(
                            self.arguments.contains_key(&key),
                            "price argument requires explicit override: {}.{}.{}",
                            pallet.name,
                            call.name,
                            argument.name
                        );
                    }
                }
            }
        }
        for key in self.arguments.keys() {
            ensure!(
                seen.contains(key),
                "stale semantic argument override {key:?}"
            );
        }
        Ok(())
    }
    fn amount_argument(&self, id: u32) -> Result<bool> {
        let ty = self.runtime.resolve(id)?;
        if matches!(
            ty.type_def,
            TypeDef::Primitive(
                scale_info::TypeDefPrimitive::U64 | scale_info::TypeDefPrimitive::U128
            )
        ) {
            return Ok(true);
        }
        if let TypeDef::Compact(compact) = &ty.type_def {
            return self.amount_argument(compact.type_param.id);
        }
        if ty.path.segments.last().is_some_and(|path| path == "Option")
            && let Some(inner) = ty.type_params.iter().find_map(|parameter| parameter.ty)
        {
            return self.amount_argument(inner.id);
        }
        Ok(false)
    }
    fn balance_argument(&self, id: u32) -> Result<bool> {
        let ty = self.runtime.resolve(id)?;
        let last = ty.path.segments.last().map(String::as_str);
        if matches!(last, Some("TaoBalance" | "AlphaBalance")) {
            return Ok(true);
        }
        if let TypeDef::Compact(compact) = &ty.type_def {
            return self.balance_argument(compact.type_param.id);
        }
        if last == Some("Option")
            && let Some(inner) = ty.type_params.iter().find_map(|p| p.ty)
        {
            return self.balance_argument(inner.id);
        }
        Ok(false)
    }
    pub fn argument(&self, pallet: &str, call: &str, field: &FieldIr) -> Result<String> {
        if self
            .arguments
            .get(&(pallet.to_owned(), call.to_owned(), field.name.clone()))
            .is_some_and(|kind| kind == "price")
        {
            let ty = self.runtime.resolve(field.type_id)?;
            return Ok(if ty.path.segments.last().is_some_and(|p| p == "Option") {
                "Option<rtao_primitives::PriceRao>"
            } else {
                "rtao_primitives::PriceRao"
            }
            .to_owned());
        }
        self.map(field.type_id)
    }
    pub fn map(&self, id: u32) -> Result<String> {
        self.map_inner(id, &mut BTreeSet::new())
    }
    fn map_inner(&self, id: u32, stack: &mut BTreeSet<u32>) -> Result<String> {
        if !stack.insert(id) {
            return Ok("rtao_core::DynValue".to_owned());
        }
        let result = self.map_node(id, stack);
        stack.remove(&id);
        result
    }
    fn map_node(&self, id: u32, stack: &mut BTreeSet<u32>) -> Result<String> {
        let ty = self.runtime.resolve(id)?;
        let full = ty.path.segments.join("::");
        let last = ty.path.segments.last().map(String::as_str).unwrap_or("");
        if let Some(rust) = self.mappings.get(&full).or_else(|| self.mappings.get(last)) {
            return Ok(rust.clone());
        }
        if full.contains("substrate_fixed")
            && matches!(last, "FixedU128" | "FixedI128" | "FixedI64")
        {
            let frac = ty
                .type_params
                .iter()
                .find(|p| p.name == "Frac")
                .and_then(|p| p.ty)
                .context("fixed type has no Frac parameter")?;
            let bits = self.typenum(frac.id, 0)?;
            return Ok(match (last, bits) {
                ("FixedU128", 32) => "rtao_primitives::U96F32",
                ("FixedU128", 64) => "rtao_primitives::U64F64",
                ("FixedI128", 32) => "rtao_primitives::I96F32",
                ("FixedI128", 64) => "rtao_primitives::I64F64",
                ("FixedI64", 32) => "rtao_primitives::I32F32",
                _ => bail!("unsupported fixed type {full} Frac={bits}"),
            }
            .to_owned());
        }
        let mut map = |id| self.map_inner(id, stack);
        if last == "Option" {
            let inner = ty
                .type_params
                .iter()
                .find_map(|p| p.ty)
                .context("Option has no payload")?;
            return Ok(format!("Option<{}>", map(inner.id)?));
        }
        if last == "BoundedVec" {
            let inner = ty
                .type_params
                .iter()
                .find(|p| p.name == "T")
                .and_then(|p| p.ty)
                .or_else(|| ty.type_params.iter().find_map(|p| p.ty))
                .context("BoundedVec has no element")?;
            return Ok(format!("Vec<{}>", map(inner.id)?));
        }
        Ok(match &ty.type_def {
            TypeDef::Primitive(p) => match p {
                scale_info::TypeDefPrimitive::Bool => "bool",
                scale_info::TypeDefPrimitive::Char => "rtao_core::DynValue",
                scale_info::TypeDefPrimitive::Str => "String",
                scale_info::TypeDefPrimitive::U8 => "u8",
                scale_info::TypeDefPrimitive::U16 => "u16",
                scale_info::TypeDefPrimitive::U32 => "u32",
                scale_info::TypeDefPrimitive::U64 => "u64",
                scale_info::TypeDefPrimitive::U128 => "u128",
                scale_info::TypeDefPrimitive::I8 => "i8",
                scale_info::TypeDefPrimitive::I16 => "i16",
                scale_info::TypeDefPrimitive::I32 => "i32",
                scale_info::TypeDefPrimitive::I64 => "i64",
                scale_info::TypeDefPrimitive::I128 => "i128",
                scale_info::TypeDefPrimitive::U256 | scale_info::TypeDefPrimitive::I256 => {
                    "rtao_core::DynValue"
                }
            }
            .to_owned(),
            TypeDef::Sequence(sequence) => format!("Vec<{}>", map(sequence.type_param.id)?),
            TypeDef::Array(array) => format!("[{}; {}]", map(array.type_param.id)?, array.len),
            TypeDef::Tuple(tuple) => format!(
                "({})",
                tuple
                    .fields
                    .iter()
                    .map(|field| map(field.id).map(|rust| format!("{rust},")))
                    .collect::<Result<Vec<_>>>()?
                    .join(" ")
            ),
            TypeDef::Compact(compact) => map(compact.type_param.id)?,
            TypeDef::Composite(_) | TypeDef::Variant(_) | TypeDef::BitSequence(_) => {
                "rtao_core::DynValue".to_owned()
            }
        })
    }
    fn typenum(&self, id: u32, depth: usize) -> Result<u32> {
        ensure!(depth < 128, "typenum recursion exceeds bound");
        let ty = self.runtime.resolve(id)?;
        let last = ty.path.segments.last().context("typenum has no path")?;
        match last.as_str() {
            "UTerm" => Ok(0),
            "B0" => Ok(0),
            "B1" => Ok(1),
            "UInt" => {
                let get = |name| {
                    ty.type_params
                        .iter()
                        .find(|p| p.name == name)
                        .and_then(|p| p.ty)
                        .with_context(|| format!("UInt missing {name}"))
                };
                let high = self.typenum(get("U")?.id, depth + 1)?;
                let bit = self.typenum(get("B")?.id, depth + 1)?;
                ensure!(bit <= 1, "typenum bit invalid");
                high.checked_mul(2)
                    .and_then(|v| v.checked_add(bit))
                    .context("typenum overflow")
            }
            _ => bail!("unrecognized typenum path {}", ty.path.segments.join("::")),
        }
    }
}

pub(crate) fn named_fields<'a>(
    runtime: &'a Runtime,
    pallet: &str,
    name: &str,
    event: bool,
) -> Result<&'a [scale_info::Field<PortableForm>]> {
    let p = runtime.pallet(pallet).context("pallet missing")?;
    let id = if event { p.events_type } else { p.calls_type }
        .with_context(|| format!("{pallet} has no variant type"))?;
    let TypeDef::Variant(variants) = &runtime.resolve(id)?.type_def else {
        bail!("{pallet} variant type is not an enum")
    };
    Ok(&variants
        .variants
        .iter()
        .find(|v| v.name == name)
        .context("variant missing")?
        .fields)
}

#[derive(Debug)]
pub(crate) struct ModelOverride {
    pub model: String,
    pub path: String,
    pub field: String,
    pub type_id: u32,
    pub rust_type: String,
}
impl TypeMapper<'_> {
    pub fn model_overrides(&self) -> Result<Vec<ModelOverride>> {
        let required = [
            "Metagraph.netuid",
            "SelectiveMetagraph.netuid",
            "Metagraph.total_stake",
            "SelectiveMetagraph.total_stake",
            "Metagraph.name",
            "Metagraph.symbol",
            "SelectiveMetagraph.name",
            "SelectiveMetagraph.symbol",
            "DynamicInfo.subnet_name",
            "DynamicInfo.token_symbol",
            "SubnetPrice.price",
        ]
        .into_iter()
        .collect::<BTreeSet<_>>();
        ensure!(
            self.fields
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>()
                == required,
            "all eleven model semantic overrides required"
        );
        let mut overrides = Vec::new();
        for (target, rust_type) in &self.fields {
            let (model, field) = target
                .split_once('.')
                .context("semantic field target requires Model.field")?;
            let types = self
                .runtime
                .types
                .types
                .iter()
                .filter(|ty| ty.ty.path.segments.last().is_some_and(|name| name == model))
                .collect::<Vec<_>>();
            ensure!(
                types.len() == 1,
                "semantic model {model} must resolve uniquely, got {}",
                types.len()
            );
            let ty = types[0];
            let TypeDef::Composite(composite) = &ty.ty.type_def else {
                bail!("semantic model {model} is not a struct")
            };
            let field_ty = composite
                .fields
                .iter()
                .find(|candidate| candidate.name.as_deref() == Some(field))
                .with_context(|| format!("semantic model field {target} absent"))?
                .ty
                .id;
            // Validate complete compact-byte/amount container shape, including optional selective fields.
            let mut inner = field_ty;
            if model == "SelectiveMetagraph" && field != "netuid" {
                let ty = self.runtime.resolve(inner)?;
                ensure!(
                    ty.path.segments.last().is_some_and(|last| last == "Option"),
                    "selective override {target} is not optional"
                );
                inner = ty
                    .type_params
                    .iter()
                    .find_map(|p| p.ty)
                    .context("Option payload absent")?
                    .id;
            }
            if field == "netuid" {
                ensure!(
                    matches!(model, "Metagraph" | "SelectiveMetagraph"),
                    "unknown packed netuid override {target}"
                );
                let TypeDef::Compact(compact) = &self.runtime.resolve(inner)?.type_def else {
                    bail!("packed metagraph netuid is not compact")
                };
                ensure!(
                    self.runtime
                        .resolve(compact.type_param.id)?
                        .path
                        .segments
                        .last()
                        .is_some_and(|path| path == "NetUid"),
                    "metagraph netuid wire identity changed"
                );
                ensure!(
                    rust_type == "rtao_primitives::NetUidStorageIndex",
                    "metagraph netuid must preserve the mechanism storage index"
                );
                overrides.push(ModelOverride {
                    model: model.to_owned(),
                    path: ty.ty.path.segments.join("::"),
                    field: field.to_owned(),
                    type_id: field_ty,
                    rust_type: rust_type.clone(),
                });
                continue;
            }
            if model == "SubnetPrice" && field == "price" {
                let wire = self.runtime.resolve(inner)?;
                ensure!(
                    matches!(
                        wire.type_def,
                        TypeDef::Primitive(scale_info::TypeDefPrimitive::U64)
                    ),
                    "subnet price wire type must be native u64"
                );
                ensure!(
                    rust_type == "rtao_primitives::PriceRao",
                    "subnet price must have explicit PriceRao semantics"
                );
                overrides.push(ModelOverride {
                    model: model.to_owned(),
                    path: ty.ty.path.segments.join("::"),
                    field: field.to_owned(),
                    type_id: field_ty,
                    rust_type: rust_type.clone(),
                });
                continue;
            }
            let TypeDef::Sequence(sequence) = &self.runtime.resolve(inner)?.type_def else {
                bail!("semantic model field {target} is not sequence")
            };
            let TypeDef::Compact(compact) = &self.runtime.resolve(sequence.type_param.id)?.type_def
            else {
                bail!("semantic model field {target} element is not compact")
            };
            let element = self.runtime.resolve(compact.type_param.id)?;
            let expected = match field {
                "total_stake" => {
                    ensure!(
                        element
                            .path
                            .segments
                            .last()
                            .is_some_and(|last| last == "TaoBalance"),
                        "stake weight override wire type changed"
                    );
                    "Vec<rtao_primitives::StakeWeight>"
                }
                "name" | "symbol" | "subnet_name" | "token_symbol" => {
                    ensure!(
                        (matches!(model, "Metagraph" | "SelectiveMetagraph")
                            && matches!(field, "name" | "symbol"))
                            || (model == "DynamicInfo"
                                && matches!(field, "subnet_name" | "token_symbol")),
                        "unknown compact text override {target}"
                    );
                    ensure!(
                        matches!(
                            element.type_def,
                            TypeDef::Primitive(scale_info::TypeDefPrimitive::U8)
                        ),
                        "compact text element changed"
                    );
                    "rtao_primitives::CompactBytesText"
                }
                _ => bail!("unrecognized semantic field override {target}"),
            };
            let expected = if model == "SelectiveMetagraph" {
                format!("Option<{expected}>")
            } else {
                expected.to_owned()
            };
            ensure!(
                rust_type == &expected,
                "semantic override {target} must be {expected}, got {rust_type}"
            );
            overrides.push(ModelOverride {
                model: model.to_owned(),
                path: ty.ty.path.segments.join("::"),
                field: field.to_owned(),
                type_id: field_ty,
                rust_type: rust_type.clone(),
            });
        }
        ensure!(
            overrides.len() == 11,
            "all eleven model semantic overrides required"
        );
        Ok(overrides)
    }
}

#[derive(Debug)]
pub(crate) struct ModelProjection {
    pub path: String,
    pub type_id: u32,
    pub projection: super::shape::Projection,
}
impl TypeMapper<'_> {
    pub fn configured_model_projections(&self) -> Result<Vec<ModelProjection>> {
        let mut result = Vec::new();
        for path in self.models.keys().chain(self.enums.keys()) {
            let types = self
                .runtime
                .types
                .types
                .iter()
                .filter(|ty| {
                    ty.ty.path.segments.join("::") == *path
                        || ty.ty.path.segments.last().is_some_and(|last| last == path)
                })
                .collect::<Vec<_>>();
            ensure!(
                types.len() == 1,
                "hand model {path} must resolve uniquely, found {}",
                types.len()
            );
            let ty = types[0];
            let projection = self
                .output_projection(ty.id)?
                .context("mapped model has no projection")?;
            result.push(ModelProjection {
                path: ty.ty.path.segments.join("::"),
                type_id: ty.id,
                projection,
            });
        }
        Ok(result)
    }
    pub fn output_projection(&self, id: u32) -> Result<Option<super::shape::Projection>> {
        if self.projections.is_empty() && self.enums.is_empty() {
            return Ok(None);
        }
        self.project(id, &mut BTreeSet::new(), 0)
    }
    fn project(
        &self,
        id: u32,
        stack: &mut BTreeSet<u32>,
        depth: usize,
    ) -> Result<Option<super::shape::Projection>> {
        use super::shape::{Projection, ProjectionField, ProjectionFields, ProjectionVariant};
        ensure!(depth < 128, "model projection exceeds recursion bound");
        if !stack.insert(id) {
            let ty = self.runtime.resolve(id)?;
            ensure!(
                !self.projections.contains_key(&ty.path.segments.join("::"))
                    && !ty
                        .path
                        .segments
                        .last()
                        .is_some_and(|last| self.projections.contains_key(last)),
                "recursive configured model projection is unsupported"
            );
            return Ok(None);
        }
        let result = (|| {
            let ty = self.runtime.resolve(id)?;
            let path = ty.path.segments.join("::");
            let last = ty.path.segments.last().map(String::as_str).unwrap_or("");
            if self.enums.contains_key(&path) || self.enums.contains_key(last) {
                ensure!(
                    matches!(ty.type_def, TypeDef::Variant(_)),
                    "exact enum {path} is not a variant type"
                );
                // An enum's complete variant set is its wire contract. A partial
                // Variant projection would incorrectly admit new variants.
                return Ok(Some(Projection::Exact));
            }
            if let Some(fields) = self
                .projections
                .get(&path)
                .or_else(|| self.projections.get(last))
            {
                let TypeDef::Composite(composite) = &ty.type_def else {
                    bail!("hand model {path} is not a by-name struct")
                };
                let mut declared = Vec::new();
                for name in fields {
                    let matches = composite
                        .fields
                        .iter()
                        .filter(|field| field.name.as_deref() == Some(name))
                        .collect::<Vec<_>>();
                    ensure!(
                        matches.len() == 1,
                        "declared model field {path}.{name} must occur once"
                    );
                    let field = matches[0];
                    let projection = self
                        .project(field.ty.id, stack, depth + 1)?
                        .unwrap_or(Projection::Exact);
                    declared.push(ProjectionField::new(name.clone(), projection));
                }
                return Ok(Some(Projection::Composite(declared)));
            }
            if last == "Option" {
                let Some(inner) = ty.type_params.iter().find_map(|parameter| parameter.ty) else {
                    bail!("Option projection missing type parameter")
                };
                if let Some(projection) = self.project(inner.id, stack, depth + 1)? {
                    return Ok(Some(Projection::Variant(vec![
                        ProjectionVariant {
                            name: "None".to_owned(),
                            fields: ProjectionFields::Exact,
                        },
                        ProjectionVariant {
                            name: "Some".to_owned(),
                            fields: ProjectionFields::Unnamed(vec![projection]),
                        },
                    ])));
                }
                return Ok(None);
            }
            Ok(match &ty.type_def {
                TypeDef::Composite(composite) if last == "BoundedVec" => {
                    ensure!(
                        composite.fields.iter().all(|field| field.name.is_none()),
                        "BoundedVec projection requires its unnamed wire wrapper"
                    );
                    let fields = composite
                        .fields
                        .iter()
                        .map(|field| self.project(field.ty.id, stack, depth + 1))
                        .collect::<Result<Vec<_>>>()?;
                    fields.iter().any(Option::is_some).then(|| {
                        Projection::CompositeUnnamed(
                            fields
                                .into_iter()
                                .map(|field| field.unwrap_or(Projection::Exact))
                                .collect(),
                        )
                    })
                }
                TypeDef::Sequence(sequence) => self
                    .project(sequence.type_param.id, stack, depth + 1)?
                    .map(|projection| Projection::Sequence(Box::new(projection))),
                TypeDef::Array(array) => self
                    .project(array.type_param.id, stack, depth + 1)?
                    .map(|projection| Projection::Array(Box::new(projection))),
                TypeDef::Compact(compact) => self
                    .project(compact.type_param.id, stack, depth + 1)?
                    .map(|projection| Projection::Compact(Box::new(projection))),
                TypeDef::Tuple(tuple) => {
                    let elements = tuple
                        .fields
                        .iter()
                        .map(|field| self.project(field.id, stack, depth + 1))
                        .collect::<Result<Vec<_>>>()?;
                    elements.iter().any(Option::is_some).then(|| {
                        Projection::Tuple(
                            elements
                                .into_iter()
                                .map(|projection| projection.unwrap_or(Projection::Exact))
                                .collect(),
                        )
                    })
                }
                _ => None,
            })
        })();
        stack.remove(&id);
        result
    }
}
