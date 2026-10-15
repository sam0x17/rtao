//! Structural metadata hashes for typed calls and by-name model projections.

use std::{collections::BTreeMap, fmt};

use blake2::{Blake2b, Digest, digest::consts::U32};
use parity_scale_codec::Encode;
use scale_info::{
    Field, PortableRegistry, Type, TypeDef, TypeDefPrimitive, Variant, form::PortableForm,
};

/// Blake2b-256 of the SCALE-encoded structural node for a metadata type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ShapeHash(pub [u8; 32]);

impl ShapeHash {
    /// Construct a generated or previously stored structural hash.
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Borrow the complete 32-byte hash.
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Display for ShapeHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

/// The three opaque outer enum IDs from V15 metadata.
///
/// These enums carry independently verified inner calls, events, and errors.
/// Their variants therefore do not contribute to a wrapper's structural hash.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OuterEnums {
    /// ID of the outer runtime call enum.
    pub call: u32,
    /// ID of the outer runtime event enum.
    pub event: u32,
    /// ID of the outer runtime error enum.
    pub error: u32,
}

impl OuterEnums {
    fn name(self, id: u32) -> Option<&'static str> {
        if id == self.call {
            Some("RuntimeCall")
        } else if id == self.event {
            Some("RuntimeEvent")
        } else if id == self.error {
            Some("RuntimeError")
        } else {
            None
        }
    }
}

/// The part of a live type a by-name model declares.
///
/// Use [`Projection::Exact`] for leaves and positional types. Named composite
/// fields are sorted before hashing; undeclared fields do not contribute.
/// Container projections apply the same rule recursively to their contents.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Projection {
    /// Include the complete structural shape of this type.
    Exact,
    /// Select named composite fields, in any order.
    Composite(Vec<ProjectionField>),
    /// Project all fields of an unnamed composite wrapper, preserving arity.
    CompositeUnnamed(Vec<Projection>),
    /// Select enum variants and recursively project their fields.
    ///
    /// Every selected variant's name and live SCALE index contribute. Additional
    /// variants do not contribute; use `Exact` if all variants must be fixed.
    Variant(Vec<ProjectionVariant>),
    /// Project each element of a sequence.
    Sequence(Box<Projection>),
    /// Project each element of an array while retaining the live array length.
    Array(Box<Projection>),
    /// Project a tuple's positional elements; its length must match exactly.
    Tuple(Vec<Projection>),
    /// Project the value wrapped by SCALE compact encoding.
    Compact(Box<Projection>),
}

/// One required named field in a model projection.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProjectionField {
    /// The live metadata field name to require.
    pub name: String,
    /// The part of that field's type declared by the model.
    pub projection: Projection,
}

impl ProjectionField {
    /// Declare a field and its recursive projection.
    pub fn new(name: impl Into<String>, projection: Projection) -> Self {
        Self {
            name: name.into(),
            projection,
        }
    }

    /// Declare a field whose entire structural type must remain unchanged.
    pub fn exact(name: impl Into<String>) -> Self {
        Self::new(name, Projection::Exact)
    }
}

/// A required enum variant in a model projection.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProjectionVariant {
    /// The live metadata variant name to require.
    pub name: String,
    /// The variant's required fields.
    pub fields: ProjectionFields,
}

/// Projection of fields within an enum variant.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ProjectionFields {
    /// Include all fields in declaration order.
    Exact,
    /// Include only the named fields declared by the model.
    Named(Vec<ProjectionField>),
    /// Include all unnamed fields with the supplied recursive projections.
    Unnamed(Vec<Projection>),
}

/// Metadata or projection failure encountered while calculating a shape.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShapeError {
    /// A referenced type is absent or its portable registry ID is invalid.
    MissingType(u32),
    /// A declared field does not exist in the live type.
    MissingField {
        /// Type whose field is missing.
        ty: u32,
        /// Required field name.
        name: String,
    },
    /// A declared enum variant does not exist in the live type.
    MissingVariant {
        /// Enum whose variant is missing.
        ty: u32,
        /// Required variant name.
        name: String,
    },
    /// A name or enum index is ambiguous, or the declaration repeats a name.
    Duplicate {
        /// Type containing the duplicate.
        ty: u32,
        /// Repeated field, variant, or index.
        name: String,
    },
    /// The live node kind or positional field count disagrees with the model.
    ProjectionMismatch {
        /// Type whose node disagrees.
        ty: u32,
        /// Explanation of the disagreement.
        detail: &'static str,
    },
    /// The metadata contains more than 128 nested non-recursive types.
    DepthLimit,
    /// A recursive metadata graph requires more than 65,536 node expansions.
    WorkLimit,
}

impl fmt::Display for ShapeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingType(ty) => write!(formatter, "metadata type {ty} is missing"),
            Self::MissingField { ty, name } => {
                write!(
                    formatter,
                    "metadata type {ty} is missing declared field {name}"
                )
            }
            Self::MissingVariant { ty, name } => {
                write!(
                    formatter,
                    "metadata type {ty} is missing declared variant {name}"
                )
            }
            Self::Duplicate { ty, name } => {
                write!(
                    formatter,
                    "metadata type {ty} has an ambiguous or repeated {name}"
                )
            }
            Self::ProjectionMismatch { ty, detail } => {
                write!(
                    formatter,
                    "metadata type {ty} does not match projection: {detail}"
                )
            }
            Self::DepthLimit => write!(formatter, "metadata type nesting exceeds 128 levels"),
            Self::WorkLimit => write!(formatter, "metadata shape traversal exceeds 65536 nodes"),
        }
    }
}

impl std::error::Error for ShapeError {}

/// Computes exact and recursively projected shapes from one portable registry.
///
/// IDs, paths, docs, type-parameter names, and source type names are excluded. SCALE
/// bit-sequence order additionally carries the semantic `Lsb0`/`Msb0` tag: those
/// order types have identical empty composite definitions but different wire
/// behavior. The tag uses the same identifier recognition as `scale-type-resolver`.
/// `substrate_fixed` numbers additionally carry validated signedness, storage
/// width, and typenum fractional-bit count. Their fraction is absent from the
/// encoded `bits` field but determines its numeric meaning. Other type parameters
/// do not contribute.
/// A recursive reference contributes `("R", depth)`, where depth is the referenced
/// ancestor's zero-based position on the current type stack. Projection traversal
/// distinguishes model nodes so an exact child of a projected recursive parent
/// still checks its complete type. Hash caches must cache complete roots, rather
/// than reusing a child's root hash.
pub struct ShapeHasher<'a> {
    registry: &'a PortableRegistry,
    outer: Option<OuterEnums>,
}

impl<'a> ShapeHasher<'a> {
    /// Hash V15 types, treating the supplied outer enum IDs as opaque.
    pub const fn new(registry: &'a PortableRegistry, outer: OuterEnums) -> Self {
        Self {
            registry,
            outer: Some(outer),
        }
    }

    /// Hash metadata that has no V15 outer enum declarations.
    pub const fn without_outer_enums(registry: &'a PortableRegistry) -> Self {
        Self {
            registry,
            outer: None,
        }
    }

    /// Hash every field, variant, and container of a type.
    pub fn exact(&self, id: u32) -> Result<ShapeHash, ShapeError> {
        Walker::new(self).shape(id, &Projection::Exact)
    }

    /// Hash only the recursively declared portion of a by-name model.
    pub fn projection(&self, id: u32, projection: &Projection) -> Result<ShapeHash, ShapeError> {
        Walker::new(self).shape(id, projection)
    }

    /// Return validated `substrate_fixed` semantics as `(signed, width, fraction)`.
    ///
    /// `width` and `fraction` count bits; `signed` is true for signed storage.
    /// A type outside this fixed-point family returns `None`. A recognized fixed
    /// type with an invalid bits field, unknown fraction representation, or a
    /// fraction outside its storage width returns an error.
    pub fn fixed_semantics(&self, id: u32) -> Result<Option<(bool, u32, u32)>, ShapeError> {
        let mut walker = Walker::new(self);
        let ty = walker.resolve(id)?;
        walker.expand()?;
        walker.fixed_semantics(id, ty)
    }

    /// Hash a call's fields as a composite node, preserving names and order.
    pub fn exact_fields(&self, fields: &[Field<PortableForm>]) -> Result<ShapeHash, ShapeError> {
        let fields = Walker::new(self).exact_fields(fields)?;
        Ok(hash_node(("C", fields)))
    }

    /// Hash positional types as a tuple node, preserving their order.
    pub fn exact_tuple(&self, ids: &[u32]) -> Result<ShapeHash, ShapeError> {
        let mut walker = Walker::new(self);
        let children = ids
            .iter()
            .map(|id| walker.shape(*id, &Projection::Exact))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(hash_node(("T", hashes(children))))
    }
}

fn hash_node(node: impl Encode) -> ShapeHash {
    ShapeHash(Blake2b::<U32>::digest(node.encode()).into())
}

fn hashes(values: Vec<ShapeHash>) -> Vec<[u8; 32]> {
    values.into_iter().map(|hash| hash.0).collect()
}

type FieldShapes = Vec<(Option<String>, [u8; 32])>;

struct Walker<'a, 'registry> {
    hasher: &'a ShapeHasher<'registry>,
    stack: Vec<(u32, Projection)>,
    acyclic_cache: BTreeMap<u32, ShapeHash>,
    recursive_references: u64,
    expansions: usize,
}

impl<'a, 'registry> Walker<'a, 'registry> {
    fn new(hasher: &'a ShapeHasher<'registry>) -> Self {
        Self {
            hasher,
            stack: Vec::new(),
            acyclic_cache: BTreeMap::new(),
            recursive_references: 0,
            expansions: 0,
        }
    }

    fn shape(&mut self, id: u32, projection: &Projection) -> Result<ShapeHash, ShapeError> {
        let ty = self.resolve(id)?;
        if let Some(name) = self.hasher.outer.and_then(|outer| outer.name(id)) {
            if !matches!(ty.type_def, TypeDef::Variant(_)) {
                return Err(ShapeError::ProjectionMismatch {
                    ty: id,
                    detail: "outer runtime type is not an enum",
                });
            }
            return Ok(hash_node(("O", name)));
        }
        if let Some(depth) = self
            .stack
            .iter()
            .position(|(ancestor, model)| *ancestor == id && model == projection)
        {
            self.recursive_references += 1;
            // Bounded below, so this conversion is independent of pointer width.
            return Ok(hash_node(("R", depth as u32)));
        }
        if self.stack.len() == 128 {
            return Err(ShapeError::DepthLimit);
        }
        if projection == &Projection::Exact
            && let Some(hash) = self.acyclic_cache.get(&id)
        {
            return Ok(*hash);
        }
        self.expand()?;
        let recursive_before = self.recursive_references;
        self.stack.push((id, projection.clone()));
        let result = self.semantic_node(id, ty, projection);
        self.stack.pop();
        if projection == &Projection::Exact
            && self.recursive_references == recursive_before
            && let Ok(hash) = result
        {
            // No back edge was encountered, so no ancestor ID, recursion depth,
            // or projection affected this subtree's hash. Reuse it across all
            // paths through an acyclic DAG rather than expanding those paths.
            self.acyclic_cache.insert(id, hash);
        }
        result
    }

    fn resolve(&self, id: u32) -> Result<&'registry Type<PortableForm>, ShapeError> {
        self.hasher
            .registry
            .types
            .get(id as usize)
            .filter(|ty| ty.id == id)
            .map(|ty| &ty.ty)
            .ok_or(ShapeError::MissingType(id))
    }

    fn expand(&mut self) -> Result<(), ShapeError> {
        if self.expansions == 65_536 {
            return Err(ShapeError::WorkLimit);
        }
        self.expansions += 1;
        Ok(())
    }

    fn semantic_node(
        &mut self,
        id: u32,
        ty: &Type<PortableForm>,
        projection: &Projection,
    ) -> Result<ShapeHash, ShapeError> {
        let fixed = self.fixed_semantics(id, ty)?;
        let structural = self.node(id, &ty.type_def, projection)?;
        Ok(match fixed {
            Some((signed, width, fraction)) => {
                hash_node(("F", signed, width, fraction, structural.0))
            }
            None => structural,
        })
    }

    fn fixed_semantics(
        &mut self,
        id: u32,
        ty: &Type<PortableForm>,
    ) -> Result<Option<(bool, u32, u32)>, ShapeError> {
        if !ty
            .path
            .segments
            .iter()
            .any(|part| part == "substrate_fixed")
        {
            return Ok(None);
        }
        let mismatch = |detail| ShapeError::ProjectionMismatch { ty: id, detail };
        let Some(name) = ty.path.segments.last().map(String::as_str) else {
            return Ok(None);
        };
        let (signed, width, primitive) = match name {
            "FixedU8" => (false, 8, TypeDefPrimitive::U8),
            "FixedU16" => (false, 16, TypeDefPrimitive::U16),
            "FixedU32" => (false, 32, TypeDefPrimitive::U32),
            "FixedU64" => (false, 64, TypeDefPrimitive::U64),
            "FixedU128" => (false, 128, TypeDefPrimitive::U128),
            "FixedI8" => (true, 8, TypeDefPrimitive::I8),
            "FixedI16" => (true, 16, TypeDefPrimitive::I16),
            "FixedI32" => (true, 32, TypeDefPrimitive::I32),
            "FixedI64" => (true, 64, TypeDefPrimitive::I64),
            "FixedI128" => (true, 128, TypeDefPrimitive::I128),
            _ if name.starts_with("Fixed") => {
                return Err(mismatch("unsupported fixed-point representation"));
            }
            _ => return Ok(None),
        };
        let TypeDef::Composite(composite) = &ty.type_def else {
            return Err(mismatch("fixed-point type is not a composite"));
        };
        let [bits] = composite.fields.as_slice() else {
            return Err(mismatch("fixed-point type must have one bits field"));
        };
        if bits.name.as_deref() != Some("bits")
            || self.resolve(bits.ty.id)?.type_def != TypeDef::Primitive(primitive)
        {
            return Err(mismatch(
                "fixed-point bits primitive disagrees with representation",
            ));
        }
        let [fraction] = ty.type_params.as_slice() else {
            return Err(mismatch(
                "fixed-point type must have one fraction parameter",
            ));
        };
        let fraction = fraction
            .ty
            .ok_or_else(|| mismatch("fixed-point fraction parameter is unresolved"))?;
        let fraction = self.typenum_unsigned(fraction.id, self.stack.len())?;
        if fraction > width {
            return Err(mismatch("fixed-point fraction exceeds storage width"));
        }
        Ok(Some((signed, width, fraction)))
    }

    fn typenum_node(
        &mut self,
        id: u32,
        depth: usize,
    ) -> Result<&'registry Type<PortableForm>, ShapeError> {
        if depth >= 128 {
            return Err(ShapeError::DepthLimit);
        }
        self.expand()?;
        let ty = self.resolve(id)?;
        if !ty
            .path
            .segments
            .iter()
            .any(|part| matches!(part.as_str(), "substrate_typenum" | "typenum"))
        {
            return Err(ShapeError::ProjectionMismatch {
                ty: id,
                detail: "unrecognized fixed-point fraction type",
            });
        }
        Ok(ty)
    }

    fn typenum_unsigned(&mut self, id: u32, depth: usize) -> Result<u32, ShapeError> {
        let ty = self.typenum_node(id, depth)?;
        let mismatch = |detail| ShapeError::ProjectionMismatch { ty: id, detail };
        let TypeDef::Composite(composite) = &ty.type_def else {
            return Err(mismatch("typenum unsigned value is not a composite"));
        };
        match ty.path.segments.last().map(String::as_str) {
            Some("UTerm") if composite.fields.is_empty() && ty.type_params.is_empty() => Ok(0),
            Some("UInt") => {
                let [high, bit] = ty.type_params.as_slice() else {
                    return Err(mismatch("typenum UInt must have two parameters"));
                };
                let (Some(high), Some(bit)) = (high.ty, bit.ty) else {
                    return Err(mismatch("typenum UInt parameter is unresolved"));
                };
                let [msb, lsb] = composite.fields.as_slice() else {
                    return Err(mismatch("typenum UInt must have msb and lsb fields"));
                };
                if msb.name.as_deref() != Some("msb")
                    || lsb.name.as_deref() != Some("lsb")
                    || msb.ty != high
                    || lsb.ty != bit
                {
                    return Err(mismatch("typenum UInt fields disagree with parameters"));
                }
                let high = self.typenum_unsigned(high.id, depth + 1)?;
                let bit = self.typenum_bit(bit.id, depth + 1)?;
                high.checked_mul(2)
                    .and_then(|value| value.checked_add(bit))
                    .ok_or_else(|| mismatch("typenum fraction overflows u32"))
            }
            _ => Err(mismatch("unrecognized typenum unsigned value")),
        }
    }

    fn typenum_bit(&mut self, id: u32, depth: usize) -> Result<u32, ShapeError> {
        let ty = self.typenum_node(id, depth)?;
        let mismatch = |detail| ShapeError::ProjectionMismatch { ty: id, detail };
        if !matches!(&ty.type_def, TypeDef::Composite(value) if value.fields.is_empty())
            || !ty.type_params.is_empty()
        {
            return Err(mismatch("typenum bit must be an empty marker"));
        }
        match ty.path.segments.last().map(String::as_str) {
            Some("B0") => Ok(0),
            Some("B1") => Ok(1),
            _ => Err(mismatch("unrecognized typenum bit")),
        }
    }

    fn node(
        &mut self,
        id: u32,
        node: &TypeDef<PortableForm>,
        projection: &Projection,
    ) -> Result<ShapeHash, ShapeError> {
        let mismatch = |detail| ShapeError::ProjectionMismatch { ty: id, detail };
        match (node, projection) {
            (TypeDef::Composite(composite), Projection::Exact) => {
                Ok(hash_node(("C", self.exact_fields(&composite.fields)?)))
            }
            (TypeDef::Composite(composite), Projection::Composite(fields)) => Ok(hash_node((
                "C",
                self.named_fields(id, &composite.fields, fields)?,
            ))),
            (TypeDef::Composite(composite), Projection::CompositeUnnamed(fields)) => {
                if composite.fields.len() != fields.len()
                    || composite.fields.iter().any(|field| field.name.is_some())
                {
                    return Err(mismatch("unnamed composite fields differ"));
                }
                let children = composite
                    .fields
                    .iter()
                    .zip(fields)
                    .map(|(field, projection)| {
                        Ok((None::<String>, self.shape(field.ty.id, projection)?.0))
                    })
                    .collect::<Result<Vec<_>, ShapeError>>()?;
                Ok(hash_node(("C", children)))
            }
            (TypeDef::Variant(variants), Projection::Exact) => {
                let sorted = self.variants(id, &variants.variants)?;
                let mut children = Vec::with_capacity(sorted.len());
                for variant in sorted {
                    children.push((
                        variant.index,
                        &variant.name,
                        self.exact_fields(&variant.fields)?,
                    ));
                }
                Ok(hash_node(("V", children)))
            }
            (TypeDef::Variant(variants), Projection::Variant(declared)) => {
                let live = self.variants(id, &variants.variants)?;
                let mut seen = BTreeMap::new();
                let mut selected = Vec::with_capacity(declared.len());
                for variant in declared {
                    if seen.insert(&variant.name, ()).is_some() {
                        return Err(ShapeError::Duplicate {
                            ty: id,
                            name: variant.name.clone(),
                        });
                    }
                    let found = live
                        .iter()
                        .find(|live| live.name == variant.name)
                        .ok_or_else(|| ShapeError::MissingVariant {
                            ty: id,
                            name: variant.name.clone(),
                        })?;
                    let fields = match &variant.fields {
                        ProjectionFields::Exact => self.exact_fields(&found.fields)?,
                        ProjectionFields::Named(fields) => {
                            self.named_fields(id, &found.fields, fields)?
                        }
                        ProjectionFields::Unnamed(fields) => {
                            if found.fields.len() != fields.len()
                                || found.fields.iter().any(|field| field.name.is_some())
                            {
                                return Err(mismatch("unnamed variant fields differ"));
                            }
                            found
                                .fields
                                .iter()
                                .zip(fields)
                                .map(|(field, projection)| {
                                    Ok((None, self.shape(field.ty.id, projection)?.0))
                                })
                                .collect::<Result<_, ShapeError>>()?
                        }
                    };
                    selected.push((found.index, &found.name, fields));
                }
                selected.sort_by_key(|variant| variant.0);
                Ok(hash_node(("V", selected)))
            }
            (TypeDef::Sequence(sequence), Projection::Exact) => Ok(hash_node((
                "S",
                self.shape(sequence.type_param.id, &Projection::Exact)?.0,
            ))),
            (TypeDef::Sequence(sequence), Projection::Sequence(child)) => Ok(hash_node((
                "S",
                self.shape(sequence.type_param.id, child)?.0,
            ))),
            (TypeDef::Array(array), Projection::Exact) => Ok(hash_node((
                "A",
                array.len,
                self.shape(array.type_param.id, &Projection::Exact)?.0,
            ))),
            (TypeDef::Array(array), Projection::Array(child)) => Ok(hash_node((
                "A",
                array.len,
                self.shape(array.type_param.id, child)?.0,
            ))),
            (TypeDef::Tuple(tuple), Projection::Exact) => {
                let children = tuple
                    .fields
                    .iter()
                    .map(|field| self.shape(field.id, &Projection::Exact))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(hash_node(("T", hashes(children))))
            }
            (TypeDef::Tuple(tuple), Projection::Tuple(fields)) => {
                if tuple.fields.len() != fields.len() {
                    return Err(mismatch("tuple element count differs"));
                }
                let children = tuple
                    .fields
                    .iter()
                    .zip(fields)
                    .map(|(field, projection)| self.shape(field.id, projection))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(hash_node(("T", hashes(children))))
            }
            (TypeDef::Compact(compact), Projection::Exact) => Ok(hash_node((
                "K",
                self.shape(compact.type_param.id, &Projection::Exact)?.0,
            ))),
            (TypeDef::Compact(compact), Projection::Compact(child)) => Ok(hash_node((
                "K",
                self.shape(compact.type_param.id, child)?.0,
            ))),
            (TypeDef::Primitive(primitive), Projection::Exact) => Ok(hash_node(("P", primitive))),
            (TypeDef::BitSequence(bits), Projection::Exact) => {
                let order_id = bits.bit_order_type.id;
                let order = self
                    .hasher
                    .registry
                    .resolve(order_id)
                    .ok_or(ShapeError::MissingType(order_id))?;
                let order_name = order
                    .path
                    .segments
                    .last()
                    .map(String::as_str)
                    .filter(|name| matches!(*name, "Lsb0" | "Msb0"))
                    .ok_or_else(|| mismatch("unsupported bit-sequence order"))?;
                let order_shape = hash_node((
                    "BO",
                    order_name,
                    self.shape(order_id, &Projection::Exact)?.0,
                ));
                Ok(hash_node((
                    "B",
                    order_shape.0,
                    self.shape(bits.bit_store_type.id, &Projection::Exact)?.0,
                )))
            }
            _ => Err(mismatch("metadata node kind differs")),
        }
    }

    fn exact_fields(&mut self, fields: &[Field<PortableForm>]) -> Result<FieldShapes, ShapeError> {
        fields
            .iter()
            .map(|field| {
                Ok((
                    field.name.clone(),
                    self.shape(field.ty.id, &Projection::Exact)?.0,
                ))
            })
            .collect()
    }

    fn named_fields(
        &mut self,
        id: u32,
        live: &[Field<PortableForm>],
        declared: &[ProjectionField],
    ) -> Result<FieldShapes, ShapeError> {
        let mut names = BTreeMap::new();
        for field in declared {
            if names.insert(&field.name, &field.projection).is_some() {
                return Err(ShapeError::Duplicate {
                    ty: id,
                    name: field.name.clone(),
                });
            }
        }
        names
            .into_iter()
            .map(|(name, projection)| {
                let mut matching = live
                    .iter()
                    .filter(|field| field.name.as_ref() == Some(name));
                let field = matching.next().ok_or_else(|| ShapeError::MissingField {
                    ty: id,
                    name: name.clone(),
                })?;
                if matching.next().is_some() {
                    return Err(ShapeError::Duplicate {
                        ty: id,
                        name: name.clone(),
                    });
                }
                Ok((Some(name.clone()), self.shape(field.ty.id, projection)?.0))
            })
            .collect()
    }

    fn variants<'node>(
        &self,
        id: u32,
        variants: &'node [Variant<PortableForm>],
    ) -> Result<Vec<&'node Variant<PortableForm>>, ShapeError> {
        let mut sorted: Vec<_> = variants.iter().collect();
        sorted.sort_by_key(|variant| variant.index);
        let mut names = BTreeMap::new();
        let mut indices = BTreeMap::new();
        for variant in &sorted {
            if names.insert(&variant.name, ()).is_some() {
                return Err(ShapeError::Duplicate {
                    ty: id,
                    name: variant.name.clone(),
                });
            }
            if indices.insert(variant.index, ()).is_some() {
                return Err(ShapeError::Duplicate {
                    ty: id,
                    name: format!("variant index {}", variant.index),
                });
            }
        }
        Ok(sorted)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use parity_scale_codec::Decode;
    use scale_info::{
        Path, PortableType, Type, TypeDefArray, TypeDefBitSequence, TypeDefCompact,
        TypeDefComposite, TypeDefPrimitive, TypeDefSequence, TypeDefTuple, TypeDefVariant,
    };

    fn registry(nodes: Vec<TypeDef<PortableForm>>) -> PortableRegistry {
        PortableRegistry {
            types: nodes
                .into_iter()
                .enumerate()
                .map(|(id, node)| {
                    PortableType::new(
                        id as u32,
                        Type::new(Path::default(), Vec::new(), node, Vec::new()),
                    )
                })
                .collect(),
        }
    }

    fn field(name: Option<&str>, id: u32) -> Field<PortableForm> {
        Field {
            name: name.map(str::to_owned),
            ty: id.into(),
            type_name: None,
            docs: Vec::new(),
        }
    }

    fn composite(fields: Vec<Field<PortableForm>>) -> TypeDef<PortableForm> {
        TypeDefComposite { fields }.into()
    }

    fn variant(name: &str, index: u8, fields: Vec<Field<PortableForm>>) -> Variant<PortableForm> {
        Variant::new(name.to_owned(), fields, index, Vec::new())
    }

    fn variants(values: Vec<Variant<PortableForm>>) -> TypeDef<PortableForm> {
        TypeDefVariant { variants: values }.into()
    }

    fn hash(registry: &PortableRegistry, id: u32) -> ShapeHash {
        ShapeHasher::without_outer_enums(registry)
            .exact(id)
            .unwrap()
    }

    fn projected(registry: &PortableRegistry, id: u32, projection: &Projection) -> ShapeHash {
        ShapeHasher::without_outer_enums(registry)
            .projection(id, projection)
            .unwrap()
    }

    #[test]
    fn node_encoding_matches_documented_scale_tuples() {
        let values = registry(vec![
            TypeDefPrimitive::U64.into(),
            composite(vec![field(Some("amount"), 0)]),
        ]);
        let primitive = hash_node(("P", TypeDefPrimitive::U64));
        assert_eq!(hash(&values, 0), primitive);
        assert_eq!(
            hash(&values, 1),
            hash_node(("C", vec![(Some("amount".to_owned()), primitive.0)]))
        );
        assert_eq!(
            ShapeHasher::without_outer_enums(&values)
                .exact_fields(&[field(Some("amount"), 0)])
                .unwrap(),
            hash(&values, 1)
        );
    }

    #[test]
    fn exact_composite_detects_field_name_order_type_and_append() {
        let original = registry(vec![
            TypeDefPrimitive::U64.into(),
            TypeDefPrimitive::U32.into(),
            composite(vec![field(Some("a"), 0), field(Some("b"), 1)]),
        ]);
        for fields in [
            vec![field(Some("renamed"), 0), field(Some("b"), 1)],
            vec![field(Some("b"), 1), field(Some("a"), 0)],
            vec![field(Some("a"), 1), field(Some("b"), 1)],
            vec![
                field(Some("a"), 0),
                field(Some("b"), 1),
                field(Some("c"), 0),
            ],
        ] {
            let mut changed = original.clone();
            changed.types[2].ty.type_def = composite(fields);
            assert_ne!(hash(&original, 2), hash(&changed, 2));
        }
    }

    #[test]
    fn paths_docs_type_params_and_source_names_do_not_contribute() {
        let original = registry(vec![
            TypeDefPrimitive::U64.into(),
            composite(vec![field(Some("amount"), 0)]),
        ]);
        let mut changed = original.clone();
        changed.types[1].ty.path.segments = vec!["renamed_crate".into(), "Renamed".into()];
        changed.types[1].ty.docs = vec!["new docs".into()];
        changed.types[1].ty.type_params = vec![scale_info::TypeParameter {
            name: "T".into(),
            ty: Some(0.into()),
        }];
        let TypeDef::Composite(fields) = &mut changed.types[1].ty.type_def else {
            panic!("composite fixture");
        };
        fields.fields[0].docs = vec!["field docs".into()];
        fields.fields[0].type_name = Some("AmountAlias".into());
        assert_eq!(hash(&original, 1), hash(&changed, 1));
    }

    #[test]
    fn variant_declaration_order_is_ignored_but_index_name_and_fields_are_exact() {
        let original = registry(vec![
            TypeDefPrimitive::U64.into(),
            variants(vec![
                variant("B", 5, vec![field(None, 0)]),
                variant("A", 2, vec![]),
            ]),
        ]);
        let mut reordered = original.clone();
        let TypeDef::Variant(variants) = &mut reordered.types[1].ty.type_def else {
            panic!("variant fixture");
        };
        variants.variants.reverse();
        assert_eq!(hash(&original, 1), hash(&reordered, 1));
        for mutation in 0..3 {
            let mut changed = original.clone();
            let TypeDef::Variant(variants) = &mut changed.types[1].ty.type_def else {
                panic!("variant fixture");
            };
            match mutation {
                0 => variants.variants[0].index = 6,
                1 => variants.variants[0].name = "Renamed".into(),
                _ => variants.variants[0].fields.clear(),
            }
            assert_ne!(hash(&original, 1), hash(&changed, 1));
        }
    }

    #[test]
    fn container_nodes_retain_encoding_and_lengths() {
        let values = registry(vec![
            TypeDefPrimitive::U8.into(),
            TypeDefPrimitive::U16.into(),
            TypeDefSequence {
                type_param: 0.into(),
            }
            .into(),
            TypeDefArray {
                len: 4,
                type_param: 0.into(),
            }
            .into(),
            TypeDefCompact {
                type_param: 0.into(),
            }
            .into(),
            TypeDefTuple {
                fields: vec![0.into(), 1.into()],
            }
            .into(),
            composite(vec![]),
            TypeDefBitSequence {
                bit_store_type: 0.into(),
                bit_order_type: 6.into(),
            }
            .into(),
        ]);
        let mut values = values;
        values.types[6].ty.path.segments = vec!["bitvec".into(), "order".into(), "Lsb0".into()];
        let u8_shape = hash(&values, 0).0;
        let u16_shape = hash(&values, 1).0;
        assert_eq!(hash(&values, 2), hash_node(("S", u8_shape)));
        assert_eq!(hash(&values, 3), hash_node(("A", 4u32, u8_shape)));
        assert_eq!(hash(&values, 4), hash_node(("K", u8_shape)));
        assert_eq!(
            hash(&values, 5),
            hash_node(("T", vec![u8_shape, u16_shape]))
        );
        assert_eq!(
            hash(&values, 7),
            hash_node((
                "B",
                hash_node(("BO", "Lsb0", hash(&values, 6).0)).0,
                u8_shape
            ))
        );
        let mut changed = values.clone();
        changed.types[3].ty.type_def = TypeDefArray {
            len: 5,
            type_param: 0.into(),
        }
        .into();
        assert_ne!(hash(&values, 3), hash(&changed, 3));
        assert_ne!(hash(&values, 0), hash(&values, 4));
    }

    #[test]
    fn bit_order_semantics_are_exact_even_when_order_types_have_the_same_structure() {
        let mut values = registry(vec![
            TypeDefPrimitive::U8.into(),
            composite(vec![]),
            composite(vec![]),
            TypeDefBitSequence {
                bit_store_type: 0.into(),
                bit_order_type: 1.into(),
            }
            .into(),
        ]);
        values.types[1].ty.path.segments = vec!["bitvec".into(), "order".into(), "Lsb0".into()];
        values.types[2].ty.path.segments = vec!["bitvec".into(), "order".into(), "Msb0".into()];
        assert_eq!(hash(&values, 1), hash(&values, 2));
        let mut changed = values.clone();
        changed.types[3].ty.type_def = TypeDefBitSequence {
            bit_store_type: 0.into(),
            bit_order_type: 2.into(),
        }
        .into();
        assert_ne!(hash(&values, 3), hash(&changed, 3));
        let mut moved = values.clone();
        moved.types[1].ty.path.segments = vec!["another".into(), "module".into(), "Lsb0".into()];
        moved.types[1].ty.docs = vec!["Order docs changed".into()];
        assert_eq!(hash(&values, 3), hash(&moved, 3));
        moved.types[1].ty.path.segments = vec!["UnsupportedOrder".into()];
        assert!(matches!(
            ShapeHasher::without_outer_enums(&moved).exact(3),
            Err(ShapeError::ProjectionMismatch { .. })
        ));
    }

    #[test]
    fn projection_ignores_appended_fields_and_live_order_recursively() {
        let original = registry(vec![
            TypeDefPrimitive::U64.into(),
            TypeDefPrimitive::U32.into(),
            composite(vec![field(Some("kept"), 0), field(Some("ignored"), 1)]),
            TypeDefSequence {
                type_param: 2.into(),
            }
            .into(),
            composite(vec![field(Some("values"), 3), field(Some("height"), 1)]),
        ]);
        let projection = Projection::Composite(vec![
            ProjectionField::new(
                "values",
                Projection::Sequence(Box::new(Projection::Composite(vec![
                    ProjectionField::exact("kept"),
                ]))),
            ),
            ProjectionField::exact("height"),
        ]);
        let mut changed = original.clone();
        changed.types[2].ty.type_def = composite(vec![
            field(Some("new"), 1),
            field(Some("ignored"), 0),
            field(Some("kept"), 0),
        ]);
        changed.types[4].ty.type_def = composite(vec![
            field(Some("height"), 1),
            field(Some("extra"), 0),
            field(Some("values"), 3),
        ]);
        assert_eq!(
            projected(&original, 4, &projection),
            projected(&changed, 4, &projection)
        );
        assert_ne!(hash(&original, 4), hash(&changed, 4));
        let Projection::Composite(mut fields) = projection.clone() else {
            panic!("projection fixture")
        };
        fields.reverse();
        assert_eq!(
            projected(&original, 4, &projection),
            projected(&original, 4, &Projection::Composite(fields))
        );
    }

    #[test]
    fn projection_rejects_missing_fields_and_changes_on_retyping() {
        let original = registry(vec![
            TypeDefPrimitive::U64.into(),
            TypeDefPrimitive::U32.into(),
            composite(vec![field(Some("amount"), 0)]),
        ]);
        let projection = Projection::Composite(vec![ProjectionField::exact("amount")]);
        let mut renamed = original.clone();
        renamed.types[2].ty.type_def = composite(vec![field(Some("renamed"), 0)]);
        assert_eq!(
            ShapeHasher::without_outer_enums(&renamed).projection(2, &projection),
            Err(ShapeError::MissingField {
                ty: 2,
                name: "amount".into()
            })
        );
        let mut retyped = original.clone();
        retyped.types[2].ty.type_def = composite(vec![field(Some("amount"), 1)]);
        assert_ne!(
            projected(&original, 2, &projection),
            projected(&retyped, 2, &projection)
        );
    }

    #[test]
    fn recursive_shapes_are_stable_after_type_id_renumbering() {
        let first = registry(vec![
            TypeDefPrimitive::U32.into(),
            composite(vec![field(Some("value"), 0), field(Some("next"), 2)]),
            variants(vec![
                variant("None", 0, vec![]),
                variant("Some", 1, vec![field(None, 1)]),
            ]),
        ]);
        let renumbered = registry(vec![
            variants(vec![
                variant("None", 0, vec![]),
                variant("Some", 1, vec![field(None, 2)]),
            ]),
            TypeDefPrimitive::U32.into(),
            composite(vec![field(Some("value"), 1), field(Some("next"), 0)]),
        ]);
        assert_eq!(hash(&first, 1), hash(&renumbered, 2));
        assert_eq!(hash(&first, 2), hash(&renumbered, 0));
    }

    #[test]
    fn shared_acyclic_graphs_are_hashed_once_per_type() {
        let mut nodes = vec![TypeDefPrimitive::U64.into()];
        let mut previous = [0, 0];
        for layer in 0..30 {
            let left = nodes.len() as u32;
            nodes.push(composite(vec![
                field(Some("left"), previous[0]),
                field(Some("right"), previous[1]),
            ]));
            let right = nodes.len() as u32;
            nodes.push(composite(vec![
                field(Some("first"), previous[0]),
                field(Some("second"), previous[1]),
            ]));
            previous = [left, right];
            assert_eq!(left, 1 + 2 * layer);
        }
        let values = registry(nodes);
        let hasher = ShapeHasher::without_outer_enums(&values);
        let mut walker = Walker::new(&hasher);
        walker.shape(previous[0], &Projection::Exact).unwrap();
        assert!(walker.expansions <= values.types.len());
    }

    #[test]
    fn recursive_graphs_have_a_work_budget_and_deep_graphs_fail_closed() {
        let mut nodes = vec![composite(vec![field(Some("cycle"), 0)])];
        let mut previous = [0, 0];
        for _ in 0..20 {
            let left = nodes.len() as u32;
            nodes.push(composite(vec![
                field(Some("left"), previous[0]),
                field(Some("right"), previous[1]),
            ]));
            let right = nodes.len() as u32;
            nodes.push(composite(vec![
                field(Some("first"), previous[0]),
                field(Some("second"), previous[1]),
            ]));
            previous = [left, right];
        }
        let values = registry(nodes);
        assert_eq!(
            ShapeHasher::without_outer_enums(&values).exact(previous[0]),
            Err(ShapeError::WorkLimit)
        );
        let mut nodes = vec![TypeDefPrimitive::U64.into()];
        for _ in 0..256 {
            nodes.push(
                TypeDefSequence {
                    type_param: (nodes.len() as u32 - 1).into(),
                }
                .into(),
            );
        }
        let deep = registry(nodes);
        assert_eq!(
            ShapeHasher::without_outer_enums(&deep).exact(256),
            Err(ShapeError::DepthLimit)
        );
    }

    #[test]
    fn recursion_depth_distinguishes_different_back_edges() {
        let original = registry(vec![
            composite(vec![field(Some("child"), 1)]),
            composite(vec![field(Some("back"), 0)]),
        ]);
        let mut changed = original.clone();
        changed.types[1].ty.type_def = composite(vec![field(Some("back"), 1)]);
        assert_ne!(hash(&original, 0), hash(&changed, 0));
    }

    #[test]
    fn exact_recursive_child_still_checks_fields_omitted_by_parent_projection() {
        let original = registry(vec![
            TypeDefPrimitive::U64.into(),
            TypeDefPrimitive::U32.into(),
            composite(vec![field(Some("value"), 0), field(Some("next"), 2)]),
        ]);
        let mut changed = original.clone();
        changed.types[2].ty.type_def =
            composite(vec![field(Some("value"), 1), field(Some("next"), 2)]);
        let projection = Projection::Composite(vec![ProjectionField::exact("next")]);
        assert_ne!(
            projected(&original, 2, &projection),
            projected(&changed, 2, &projection)
        );
    }

    #[test]
    fn outer_enums_are_opaque_and_distinct_without_using_paths() {
        let original = registry(vec![
            variants(vec![variant("Existing", 0, vec![])]),
            variants(vec![variant("Event", 0, vec![])]),
            variants(vec![variant("Error", 0, vec![])]),
            TypeDefSequence {
                type_param: 0.into(),
            }
            .into(),
            composite(vec![field(Some("calls"), 3)]),
        ]);
        let outer = OuterEnums {
            call: 0,
            event: 1,
            error: 2,
        };
        let mut changed = original.clone();
        changed.types[0].ty.type_def = variants(vec![
            variant("Existing", 0, vec![]),
            variant("NewUnrelatedCall", 1, vec![field(None, 2)]),
        ]);
        let first = ShapeHasher::new(&original, outer);
        let second = ShapeHasher::new(&changed, outer);
        assert_eq!(first.exact(4).unwrap(), second.exact(4).unwrap());
        assert_eq!(first.exact(0).unwrap(), hash_node(("O", "RuntimeCall")));
        assert_ne!(first.exact(0).unwrap(), first.exact(1).unwrap());
        assert_ne!(hash(&original, 4), hash(&changed, 4));
    }

    #[test]
    fn pinned_wrapper_calls_survive_an_unrelated_pallet_call_addition() {
        let frame_metadata::RuntimeMetadataPrefixed(_, metadata) =
            frame_metadata::RuntimeMetadataPrefixed::decode(
                &mut &include_bytes!("../../../vendor/metadata/finney/478.v15.scale")[..],
            )
            .unwrap();
        let frame_metadata::RuntimeMetadata::V15(metadata) = metadata else {
            panic!("pinned V15 metadata");
        };
        let outer = OuterEnums {
            call: metadata.outer_enums.call_enum_ty.id,
            event: metadata.outer_enums.event_enum_ty.id,
            error: metadata.outer_enums.error_enum_ty.id,
        };
        let mut changed = metadata.types.clone();
        let unrelated = metadata
            .pallets
            .iter()
            .find(|pallet| pallet.name == "Balances")
            .unwrap()
            .calls
            .as_ref()
            .unwrap()
            .ty
            .id;
        let TypeDef::Variant(calls) = &mut changed.types[unrelated as usize].ty.type_def else {
            panic!("pallet call enum");
        };
        let new_index = (0..=u8::MAX)
            .find(|index| !calls.variants.iter().any(|call| call.index == *index))
            .unwrap();
        calls
            .variants
            .push(variant("new_unrelated_call", new_index, vec![]));
        let before = ShapeHasher::new(&metadata.types, outer);
        let after = ShapeHasher::new(&changed, outer);
        assert_ne!(
            before.exact(unrelated).unwrap(),
            after.exact(unrelated).unwrap()
        );
        for (pallet, call) in [
            ("Utility", "batch_all"),
            ("Multisig", "as_multi"),
            ("Proxy", "proxy_announced"),
            ("Sudo", "sudo"),
        ] {
            let ty = metadata
                .pallets
                .iter()
                .find(|candidate| candidate.name == pallet)
                .unwrap()
                .calls
                .as_ref()
                .unwrap()
                .ty
                .id;
            let TypeDef::Variant(variants) = &metadata.types.resolve(ty).unwrap().type_def else {
                panic!("pallet call enum");
            };
            let fields = &variants
                .variants
                .iter()
                .find(|variant| variant.name == call)
                .unwrap()
                .fields;
            assert_eq!(
                before.exact_fields(fields).unwrap(),
                after.exact_fields(fields).unwrap(),
                "{pallet}.{call}"
            );
        }
        // The mutation really reaches these wrapper arguments if outer enums
        // are structural; the stability assertion cannot pass vacuously.
        let structural_before = ShapeHasher::without_outer_enums(&metadata.types);
        let structural_after = ShapeHasher::without_outer_enums(&changed);
        let utility = metadata
            .pallets
            .iter()
            .find(|pallet| pallet.name == "Utility")
            .unwrap()
            .calls
            .as_ref()
            .unwrap()
            .ty
            .id;
        assert_ne!(
            structural_before.exact(utility).unwrap(),
            structural_after.exact(utility).unwrap()
        );
    }

    #[test]
    fn malformed_types_projections_and_variant_indices_fail_closed() {
        let mut values = registry(vec![
            TypeDefPrimitive::U64.into(),
            composite(vec![field(Some("amount"), 0), field(Some("amount"), 0)]),
            variants(vec![variant("A", 0, vec![]), variant("B", 0, vec![])]),
        ]);
        let hasher = ShapeHasher::without_outer_enums(&values);
        assert_eq!(hasher.exact(999), Err(ShapeError::MissingType(999)));
        assert!(matches!(hasher.exact(2), Err(ShapeError::Duplicate { .. })));
        assert!(matches!(
            hasher.projection(
                1,
                &Projection::Composite(vec![ProjectionField::exact("amount")])
            ),
            Err(ShapeError::Duplicate { .. })
        ));
        assert!(matches!(
            hasher.projection(0, &Projection::Composite(vec![])),
            Err(ShapeError::ProjectionMismatch { .. })
        ));
        values.types[0].id = 10;
        assert_eq!(
            ShapeHasher::without_outer_enums(&values).exact(0),
            Err(ShapeError::MissingType(0))
        );
    }

    #[test]
    fn projection_keeps_variant_indices_tuple_arity_and_array_lengths() {
        let values = registry(vec![
            TypeDefPrimitive::U64.into(),
            composite(vec![field(Some("amount"), 0), field(Some("ignored"), 0)]),
            variants(vec![
                variant("Some", 1, vec![field(None, 1)]),
                variant("None", 0, vec![]),
            ]),
            TypeDefTuple {
                fields: vec![1.into()],
            }
            .into(),
            TypeDefArray {
                len: 2,
                type_param: 1.into(),
            }
            .into(),
            TypeDefCompact {
                type_param: 0.into(),
            }
            .into(),
        ]);
        let child = Projection::Composite(vec![ProjectionField::exact("amount")]);
        let projection = Projection::Variant(vec![
            ProjectionVariant {
                name: "Some".into(),
                fields: ProjectionFields::Unnamed(vec![child.clone()]),
            },
            ProjectionVariant {
                name: "None".into(),
                fields: ProjectionFields::Exact,
            },
        ]);
        let mut changed = values.clone();
        changed.types[1].ty.type_def =
            composite(vec![field(Some("amount"), 0), field(Some("added"), 0)]);
        assert_eq!(
            projected(&values, 2, &projection),
            projected(&changed, 2, &projection)
        );
        let TypeDef::Variant(variants) = &mut changed.types[2].ty.type_def else {
            panic!("variant fixture")
        };
        variants.variants[0].index = 2;
        assert_ne!(
            projected(&values, 2, &projection),
            projected(&changed, 2, &projection)
        );
        assert_eq!(
            projected(&values, 3, &Projection::Tuple(vec![child.clone()])),
            projected(&changed, 3, &Projection::Tuple(vec![child.clone()]))
        );
        assert_eq!(
            projected(&values, 4, &Projection::Array(Box::new(child.clone()))),
            projected(&changed, 4, &Projection::Array(Box::new(child)))
        );
        assert_eq!(
            projected(
                &values,
                5,
                &Projection::Compact(Box::new(Projection::Exact))
            ),
            hash(&values, 5)
        );
        assert!(matches!(
            ShapeHasher::without_outer_enums(&values).projection(3, &Projection::Tuple(vec![])),
            Err(ShapeError::ProjectionMismatch { .. })
        ));
    }

    #[test]
    fn unnamed_wrappers_project_nested_models_and_keep_wire_arity() {
        let original = registry(vec![
            TypeDefPrimitive::U64.into(),
            composite(vec![field(Some("amount"), 0)]),
            composite(vec![field(None, 1)]),
        ]);
        let projection = Projection::CompositeUnnamed(vec![Projection::Composite(vec![
            ProjectionField::exact("amount"),
        ])]);
        let mut appended = original.clone();
        let TypeDef::Composite(model) = &mut appended.types[1].ty.type_def else {
            unreachable!()
        };
        model.fields.push(field(Some("future_unused"), 0));
        assert_ne!(hash(&original, 2), hash(&appended, 2));
        assert_eq!(
            projected(&original, 2, &projection),
            projected(&appended, 2, &projection)
        );
        assert!(
            ShapeHasher::without_outer_enums(&original)
                .projection(2, &Projection::CompositeUnnamed(Vec::new()))
                .is_err()
        );
        let mut named = original.clone();
        let TypeDef::Composite(wrapper) = &mut named.types[2].ty.type_def else {
            unreachable!()
        };
        wrapper.fields[0].name = Some("changed_wrapper".to_owned());
        assert!(
            ShapeHasher::without_outer_enums(&named)
                .projection(2, &projection)
                .is_err()
        );
    }

    fn pinned_runtime() -> rtao_core::runtime::Runtime {
        rtao_core::runtime::Runtime::parse(
            include_bytes!("../../../vendor/metadata/finney/478.v15.scale"),
            478,
            1,
            42,
        )
        .unwrap()
    }

    fn metagraph_fixed_ids(registry: &PortableRegistry) -> (u32, u32, u32, u32) {
        let metagraph = registry
            .types
            .iter()
            .find(|ty| {
                ty.ty
                    .path
                    .segments
                    .last()
                    .is_some_and(|name| name == "Metagraph")
            })
            .unwrap();
        let TypeDef::Composite(fields) = &metagraph.ty.type_def else {
            panic!("pinned Metagraph composite");
        };
        let fixed = fields
            .fields
            .iter()
            .find(|field| field.name.as_deref() == Some("moving_price"))
            .unwrap()
            .ty
            .id;
        let fraction32 = registry.resolve(fixed).unwrap().type_params[0]
            .ty
            .unwrap()
            .id;
        let fraction64 = registry
            .types
            .iter()
            .find(|ty| {
                ty.ty.path.segments == ["substrate_fixed", "FixedI128"]
                    && ty.ty.type_params[0].ty.unwrap().id != fraction32
            })
            .unwrap()
            .ty
            .type_params[0]
            .ty
            .unwrap()
            .id;
        let hasher = ShapeHasher::without_outer_enums(registry);
        let mut walker = Walker::new(&hasher);
        assert_eq!(walker.typenum_unsigned(fraction32, 0).unwrap(), 32);
        assert_eq!(walker.typenum_unsigned(fraction64, 0).unwrap(), 64);
        (metagraph.id, fixed, fraction32, fraction64)
    }

    fn moving_price_projection() -> Projection {
        Projection::Composite(vec![ProjectionField::exact("moving_price")])
    }

    #[test]
    fn fixed_fraction_changes_full_and_projected_metagraph_shapes() {
        let core = pinned_runtime();
        let (metagraph, fixed, _, fraction64) = metagraph_fixed_ids(&core.types);
        let mut changed = core.types.clone();
        changed.types[fixed as usize].ty.type_params[0].ty = Some(fraction64.into());
        // Fraction is the only mutation: both fixed types encode the same i128 bits.
        assert_eq!(
            core.types.resolve(fixed).unwrap().type_def,
            changed.resolve(fixed).unwrap().type_def
        );
        assert_eq!(
            ShapeHasher::without_outer_enums(&core.types).fixed_semantics(fixed),
            Ok(Some((true, 128, 32)))
        );
        assert_eq!(
            ShapeHasher::without_outer_enums(&changed).fixed_semantics(fixed),
            Ok(Some((true, 128, 64)))
        );
        for projection in [Projection::Exact, moving_price_projection()] {
            assert_ne!(
                projected(&core.types, metagraph, &projection),
                projected(&changed, metagraph, &projection)
            );
        }
        assert_ne!(hash(&core.types, metagraph), hash(&changed, metagraph));
    }

    #[test]
    fn pinned_fixed_semantics_validate_width_sign_and_ignore_parameter_names_and_docs() {
        let core = pinned_runtime();
        let (metagraph, fixed, _, _) = metagraph_fixed_ids(&core.types);
        let hasher = ShapeHasher::without_outer_enums(&core.types);
        let mut fixed_count = 0;
        for ty in &core.types.types {
            if ty
                .ty
                .path
                .segments
                .first()
                .is_some_and(|path| path == "substrate_fixed")
            {
                let mut walker = Walker::new(&hasher);
                let (signed, width, fraction) = hasher.fixed_semantics(ty.id).unwrap().unwrap();
                let structural = walker
                    .node(ty.id, &ty.ty.type_def, &Projection::Exact)
                    .unwrap();
                assert_eq!(
                    hash(&core.types, ty.id),
                    hash_node(("F", signed, width, fraction, structural.0))
                );
                assert!(matches!(
                    (signed, width, fraction),
                    (false, 128, 32 | 64) | (true, 128, 32 | 64) | (true, 64, 32)
                ));
                fixed_count += 1;
            }
        }
        assert_eq!(fixed_count, 5);
        assert_eq!(hasher.fixed_semantics(metagraph), Ok(None));
        assert_eq!(
            hasher.fixed_semantics(u32::MAX),
            Err(ShapeError::MissingType(u32::MAX))
        );
        let mut renamed = core.types.clone();
        for ty in &mut renamed.types {
            ty.ty.docs.push("Updated docs".into());
            for parameter in &mut ty.ty.type_params {
                parameter.name = "RenamedParameter".into();
            }
            if let TypeDef::Composite(composite) = &mut ty.ty.type_def {
                for field in &mut composite.fields {
                    field.type_name = Some("RenamedSourceAlias".into());
                    field.docs.push("Updated field docs".into());
                }
            }
            // The semantic family/marker identifiers remain; module locations do not matter.
            if ty.ty.path.segments.first().is_some_and(|path| {
                matches!(path.as_str(), "substrate_fixed" | "substrate_typenum")
            }) {
                ty.ty.path.segments.insert(1, "moved_module".into());
            } else {
                ty.ty.path.segments = vec!["ordinary_alias".into()];
            }
        }
        assert_eq!(hash(&core.types, fixed), hash(&renamed, fixed));
        assert_eq!(hash(&core.types, metagraph), hash(&renamed, metagraph));
        assert_eq!(
            projected(&core.types, metagraph, &moving_price_projection()),
            projected(&renamed, metagraph, &moving_price_projection())
        );
    }

    #[test]
    fn unknown_or_malformed_fixed_fraction_metadata_fails_closed() {
        let core = pinned_runtime();
        let (metagraph, fixed, fraction32, _) = metagraph_fixed_ids(&core.types);
        let TypeDef::Composite(fraction_fields) = &core.types.resolve(fraction32).unwrap().type_def
        else {
            panic!("pinned UInt composite");
        };
        let bit = fraction_fields.fields[1].ty.id;
        for mutation in 0..12 {
            let mut changed = core.types.clone();
            match mutation {
                0 => changed.types[fixed as usize].ty.type_params.clear(),
                1 => changed.types[fixed as usize].ty.type_params[0].ty = None,
                2 => changed.types[fixed as usize].ty.type_params[0].ty = Some(u32::MAX.into()),
                3 => {
                    changed.types[fraction32 as usize].ty.path.segments = vec!["UnknownFrac".into()]
                }
                4 => changed.types[fraction32 as usize].ty.type_def = TypeDefPrimitive::U32.into(),
                5 => changed.types[fraction32 as usize].ty.type_params[0].ty = None,
                6 => {
                    changed.types[fraction32 as usize].ty.type_params[1].ty =
                        Some(fraction32.into())
                }
                7 => {
                    changed.types[bit as usize].ty.path.segments =
                        vec!["substrate_typenum".into(), "B2".into()]
                }
                8 => changed.types[bit as usize].ty.type_def = composite(vec![field(None, bit)]),
                9 => changed.types[fraction32 as usize].id = u32::MAX,
                10 => {
                    changed.types[fixed as usize].ty.type_def =
                        composite(vec![field(Some("bits"), fraction32)])
                }
                _ => {
                    let duplicate = changed.types[fixed as usize].ty.type_params[0].clone();
                    changed.types[fixed as usize].ty.type_params.push(duplicate);
                }
            }
            assert!(
                ShapeHasher::without_outer_enums(&changed)
                    .exact(metagraph)
                    .is_err(),
                "mutation {mutation}"
            );
            assert!(
                ShapeHasher::without_outer_enums(&changed)
                    .projection(metagraph, &moving_price_projection())
                    .is_err(),
                "mutation {mutation}"
            );
            assert!(
                ShapeHasher::without_outer_enums(&changed)
                    .fixed_semantics(fixed)
                    .is_err(),
                "mutation {mutation}"
            );
        }
    }

    fn append_typenum_digit(registry: &mut PortableRegistry, high: u32, bit: u32) -> u32 {
        let mut value = registry
            .types
            .iter()
            .find(|ty| {
                ty.ty
                    .path
                    .segments
                    .last()
                    .is_some_and(|name| name == "UInt")
            })
            .unwrap()
            .clone();
        value.id = registry.types.len() as u32;
        value.ty.type_params[0].ty = Some(high.into());
        value.ty.type_params[1].ty = Some(bit.into());
        let TypeDef::Composite(composite) = &mut value.ty.type_def else {
            panic!("pinned UInt composite");
        };
        composite.fields[0].ty = high.into();
        composite.fields[1].ty = bit.into();
        let id = value.id;
        registry.types.push(value);
        id
    }

    #[test]
    fn fixed_fraction_parsing_enforces_range_depth_work_and_overflow_bounds() {
        let core = pinned_runtime();
        let (_, fixed, fraction32, fraction64) = metagraph_fixed_ids(&core.types);
        let TypeDef::Composite(composite) = &core.types.resolve(fraction32).unwrap().type_def
        else {
            panic!("pinned UInt composite");
        };
        let zero = composite.fields[1].ty.id;
        let mut changed = core.types.clone();
        let fraction128 = append_typenum_digit(&mut changed, fraction64, zero);
        changed.types[fixed as usize].ty.type_params[0].ty = Some(fraction128.into());
        assert!(
            ShapeHasher::without_outer_enums(&changed)
                .exact(fixed)
                .is_ok()
        );
        let fraction256 = append_typenum_digit(&mut changed, fraction128, zero);
        changed.types[fixed as usize].ty.type_params[0].ty = Some(fraction256.into());
        assert!(matches!(
            ShapeHasher::without_outer_enums(&changed).exact(fixed),
            Err(ShapeError::ProjectionMismatch {
                detail: "fixed-point fraction exceeds storage width",
                ..
            })
        ));

        let mut high = fraction256;
        for _ in 0..24 {
            high = append_typenum_digit(&mut changed, high, zero);
        }
        changed.types[fixed as usize].ty.type_params[0].ty = Some(high.into());
        assert!(matches!(
            ShapeHasher::without_outer_enums(&changed).exact(fixed),
            Err(ShapeError::ProjectionMismatch {
                detail: "typenum fraction overflows u32",
                ..
            })
        ));

        let mut cyclic = core.types.clone();
        cyclic.types[fraction32 as usize].ty.type_params[0].ty = Some(fraction32.into());
        let TypeDef::Composite(composite) = &mut cyclic.types[fraction32 as usize].ty.type_def
        else {
            panic!("pinned UInt composite");
        };
        composite.fields[0].ty = fraction32.into();
        assert_eq!(
            ShapeHasher::without_outer_enums(&cyclic).exact(fixed),
            Err(ShapeError::DepthLimit)
        );

        let hasher = ShapeHasher::without_outer_enums(&core.types);
        let mut walker = Walker::new(&hasher);
        walker.expansions = 65_535;
        assert_eq!(
            walker.shape(fixed, &Projection::Exact),
            Err(ShapeError::WorkLimit)
        );
    }
}
