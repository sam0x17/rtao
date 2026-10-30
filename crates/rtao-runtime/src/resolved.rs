//! Name-resolved call composition validated by structural shape (D-088).
//!
//! A compiled [`crate::Call`] refuses whenever the live argument shape hash
//! differs from the build's, even when the change cannot alter what the call
//! encodes: an integer moved into a single-field newtype, `Vec` replaced by
//! `BoundedVec`, a compact switch, a widened integer or reordered fields. This
//! module resolves a call and its fields by name against live metadata and
//! accepts exactly those changes. Each supplied argument declares a structural
//! [`ArgClass`]; the live field type must fit it once single-field composites
//! and `Compact` are removed. Anything else, such as an added, removed or
//! renamed field, an `Option`, an enum, a narrowed or signed integer, a fixed
//! array or a nested runtime call, refuses with a per-field
//! [`ComposeError::ResolvedIncompatible`].
//!
//! Encoding still goes through the one core encoder,
//! `Runtime::compose_call`. Its input coercions are lenient (extra dictionary
//! keys are ignored, a bare value becomes `Some`), so the result is imported
//! through the strict [`CallBytes::decode`] and every argument is decoded
//! back by its class and compared with the supplied value before the bytes
//! are returned.

use std::collections::BTreeSet;
use std::fmt;

use rtao_core::CoreError;
use rtao_core::codec::Value;
use rtao_core::codec::decode::{Cursor, compact_len, compact_u128};
use rtao_core::runtime::Runtime;
use scale_info::{Field, Type, TypeDef, TypeDefPrimitive, form::PortableForm};

use crate::handle::{CallBytes, ComposeError, MetadataLevel, RuntimeHandle};
use crate::resolved_read::scaled_wrapper;

/// Deepest chain of transparent layers followed for one argument.
const MAX_LAYERS: usize = 16;

/// Structural class one name-resolved argument must fit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArgClass {
    /// An unsigned integer of at least this many bits, fixed width or compact.
    Uint(u8),
    /// A length-prefixed byte sequence (`Vec<u8>`, `BoundedVec<u8, _>`).
    Bytes,
    /// A length-prefixed sequence whose elements fit the inner class.
    Seq(Box<ArgClass>),
}

impl fmt::Display for ArgClass {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Uint(bits) => write!(formatter, "an unsigned integer of at least {bits} bits"),
            Self::Bytes => formatter.write_str("a byte sequence"),
            Self::Seq(inner) => write!(formatter, "a sequence of {inner}"),
        }
    }
}

/// Value supplied for, or decoded from, one name-resolved argument.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArgValue {
    /// An unsigned integer.
    Uint(u128),
    /// Raw bytes of a byte sequence.
    Bytes(Vec<u8>),
    /// Elements of a sequence.
    Seq(Vec<ArgValue>),
}

impl ArgValue {
    /// A sequence of unsigned integers.
    pub fn uints<T: Copy + Into<u128>>(values: &[T]) -> Self {
        Self::Seq(
            values
                .iter()
                .map(|value| Self::Uint((*value).into()))
                .collect(),
        )
    }

    /// The integer, when this is one.
    pub fn as_uint(&self) -> Option<u128> {
        match self {
            Self::Uint(value) => Some(*value),
            _ => None,
        }
    }
}

/// One argument of a name-resolved call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedArg<'a> {
    /// Live field name.
    pub name: &'a str,
    /// Structural class the live field type must fit.
    pub class: ArgClass,
    /// Value to encode; it must fit `class`.
    pub value: ArgValue,
}

/// One field of an imported call, read back by name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InspectedField {
    /// Live field name; empty for an unnamed field.
    pub name: String,
    /// Decoded value for a field whose class the caller supplied.
    pub value: Option<ArgValue>,
    /// Whether the field's type can carry a runtime call.
    pub nests_call: bool,
}

/// An imported call's live pallet, call name and fields.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InspectedCall {
    /// Live pallet name.
    pub pallet: String,
    /// Live call name.
    pub call: String,
    /// Fields in live order.
    pub fields: Vec<InspectedField>,
}

impl InspectedCall {
    /// The field named `name`, when the live call has it.
    pub fn field(&self, name: &str) -> Option<&InspectedField> {
        self.fields.iter().find(|field| field.name == name)
    }
}

/// How one live field type carries a class, after transparent layers.
#[derive(Clone, Debug)]
enum Layout {
    Uint {
        bits: u8,
        compact: bool,
    },
    Bytes,
    Seq(Box<Layout>),
    Wrap {
        name: Option<String>,
        inner: Box<Layout>,
    },
}

fn incompatible(pallet: &str, call: &str, field: Option<&str>, detail: String) -> ComposeError {
    ComposeError::ResolvedIncompatible {
        pallet: pallet.into(),
        call: call.into(),
        field: field.map(Into::into),
        detail,
    }
}

fn describe(ty: &Type<PortableForm>) -> String {
    let path = ty.path.segments.join("::");
    let kind = match &ty.type_def {
        TypeDef::Variant(_) if ty.path.segments.last().is_some_and(|last| last == "Option") => {
            "an Option".to_owned()
        }
        TypeDef::Variant(_) => "an enum".to_owned(),
        TypeDef::Composite(composite) => format!("a struct of {} fields", composite.fields.len()),
        TypeDef::Sequence(_) => "a sequence".to_owned(),
        TypeDef::Array(array) => format!("a fixed array of {}", array.len),
        TypeDef::Tuple(tuple) => format!("a tuple of {}", tuple.fields.len()),
        TypeDef::Primitive(primitive) => format!("{primitive:?}").to_lowercase(),
        TypeDef::Compact(_) => "a compact integer".to_owned(),
        TypeDef::BitSequence(_) => "a bit sequence".to_owned(),
    };
    if path.is_empty() {
        kind
    } else {
        format!("{kind} ({path})")
    }
}

fn unsigned_bits(primitive: &TypeDefPrimitive) -> Option<u8> {
    match primitive {
        TypeDefPrimitive::U8 => Some(8),
        TypeDefPrimitive::U16 => Some(16),
        TypeDefPrimitive::U32 => Some(32),
        TypeDefPrimitive::U64 => Some(64),
        TypeDefPrimitive::U128 => Some(128),
        _ => None,
    }
}

fn resolve(core: &Runtime, id: u32) -> Result<&Type<PortableForm>, String> {
    core.resolve(id).map_err(|error| error.to_string())
}

/// Width of a compact integer's payload, through single-field composites.
fn compact_bits(core: &Runtime, mut id: u32) -> Result<u8, String> {
    for _ in 0..MAX_LAYERS {
        let ty = resolve(core, id)?;
        if let Some(detail) = scaled_wrapper(ty) {
            return Err(format!("is a compact {detail}"));
        }
        match &ty.type_def {
            TypeDef::Primitive(primitive) => {
                return unsigned_bits(primitive)
                    .ok_or_else(|| format!("is a compact {}", describe(ty)));
            }
            TypeDef::Composite(composite) if composite.fields.len() == 1 => {
                id = composite.fields[0].ty.id;
            }
            _ => return Err(format!("is a compact {}", describe(ty))),
        }
    }
    Err(format!("nests more than {MAX_LAYERS} layers"))
}

fn layout(core: &Runtime, id: u32, class: &ArgClass, depth: usize) -> Result<Layout, String> {
    if depth >= MAX_LAYERS {
        return Err(format!("nests more than {MAX_LAYERS} layers"));
    }
    if core.extrinsic.call_type == Some(id) {
        return Err("is a nested runtime call".into());
    }
    let ty = resolve(core, id)?;
    let mismatch = || format!("is {}, expected {class}", describe(ty));
    // A ratio or fixed-point wrapper changes what its integer means, so it is
    // never unwrapped to encode a plain integer argument.
    if let Some(detail) = scaled_wrapper(ty) {
        return Err(format!("is a {detail}, expected {class}"));
    }
    match (&ty.type_def, class) {
        (TypeDef::Composite(composite), _) if composite.fields.len() == 1 => {
            let field = &composite.fields[0];
            Ok(Layout::Wrap {
                name: field.name.clone(),
                inner: Box::new(layout(core, field.ty.id, class, depth + 1)?),
            })
        }
        (TypeDef::Primitive(primitive), ArgClass::Uint(min)) => match unsigned_bits(primitive) {
            Some(bits) if bits >= *min => Ok(Layout::Uint {
                bits,
                compact: false,
            }),
            Some(bits) => Err(format!("narrowed to u{bits}, expected {class}")),
            None => Err(mismatch()),
        },
        (TypeDef::Compact(compact), ArgClass::Uint(min)) => {
            let bits = compact_bits(core, compact.type_param.id)?;
            if bits < *min {
                return Err(format!("narrowed to compact u{bits}, expected {class}"));
            }
            Ok(Layout::Uint {
                bits,
                compact: true,
            })
        }
        (TypeDef::Sequence(sequence), ArgClass::Bytes) => {
            match &resolve(core, sequence.type_param.id)?.type_def {
                TypeDef::Primitive(TypeDefPrimitive::U8) => Ok(Layout::Bytes),
                _ => Err(mismatch()),
            }
        }
        (TypeDef::Sequence(sequence), ArgClass::Seq(inner)) => Ok(Layout::Seq(Box::new(
            layout(core, sequence.type_param.id, inner, depth + 1)
                .map_err(|detail| format!("element {detail}"))?,
        ))),
        _ => Err(mismatch()),
    }
}

fn fits(class: &ArgClass, value: &ArgValue) -> bool {
    match (class, value) {
        (ArgClass::Uint(bits), ArgValue::Uint(value)) => {
            *bits >= 128 || *value < (1u128 << u32::from(*bits))
        }
        (ArgClass::Bytes, ArgValue::Bytes(_)) => true,
        (ArgClass::Seq(inner), ArgValue::Seq(items)) => items.iter().all(|item| fits(inner, item)),
        _ => false,
    }
}

/// Core encoder input for `value` under `layout`. Named single-field
/// composites take a dictionary; an unnamed one around a sequence takes one
/// explicit list level, which the core's legacy newtype input removes.
fn core_value(layout: &Layout, value: &ArgValue) -> Option<Value> {
    Some(match (layout, value) {
        (Layout::Uint { .. }, ArgValue::Uint(value)) => {
            i128::try_from(*value).map_or(Value::Uint(*value), Value::Int)
        }
        (Layout::Bytes, ArgValue::Bytes(bytes)) => Value::Bytes(bytes.clone()),
        (Layout::Seq(inner), ArgValue::Seq(items)) => Value::List(
            items
                .iter()
                .map(|item| core_value(inner, item))
                .collect::<Option<_>>()?,
        ),
        (
            Layout::Wrap {
                name: Some(name),
                inner,
            },
            value,
        ) => Value::record(vec![(name.clone(), core_value(inner, value)?)]),
        (Layout::Wrap { name: None, inner }, value) => {
            let inner_value = core_value(inner, value)?;
            if matches!(**inner, Layout::Seq(_)) {
                Value::List(vec![inner_value])
            } else {
                inner_value
            }
        }
        _ => return None,
    })
}

fn read(layout: &Layout, cursor: &mut Cursor<'_>) -> Result<ArgValue, CoreError> {
    match layout {
        Layout::Uint { bits, compact } => {
            let value = if *compact {
                compact_u128(cursor)?
            } else {
                let bytes = cursor.take(usize::from(*bits / 8))?;
                let mut buf = [0u8; 16];
                buf[..bytes.len()].copy_from_slice(bytes);
                u128::from_le_bytes(buf)
            };
            if *bits < 128 && value >> u32::from(*bits) != 0 {
                return Err(CoreError::Codec(format!(
                    "compact value exceeds its u{bits} type"
                )));
            }
            Ok(ArgValue::Uint(value))
        }
        Layout::Bytes => {
            let len = compact_len(cursor)?;
            Ok(ArgValue::Bytes(cursor.take(len)?.to_vec()))
        }
        Layout::Seq(inner) => {
            let len = compact_len(cursor)?;
            // Every layout consumes at least one byte per element.
            if len > cursor.remaining() {
                return Err(CoreError::Codec(
                    "sequence length exceeds the call bytes".into(),
                ));
            }
            (0..len)
                .map(|_| read(inner, cursor))
                .collect::<Result<_, _>>()
                .map(ArgValue::Seq)
        }
        Layout::Wrap { inner, .. } => read(inner, cursor),
    }
}

/// Whether type `id` can carry the outer runtime call anywhere inside it.
fn nests_call(core: &Runtime, id: u32) -> Result<bool, String> {
    let Some(call_type) = core.extrinsic.call_type else {
        return Ok(false);
    };
    let mut seen = BTreeSet::new();
    let mut stack = vec![id];
    while let Some(id) = stack.pop() {
        if id == call_type {
            return Ok(true);
        }
        if !seen.insert(id) {
            continue;
        }
        if seen.len() > 4096 {
            return Err("argument type graph is too large to inspect".into());
        }
        let ty = resolve(core, id)?;
        let fields = |fields: &[Field<PortableForm>], stack: &mut Vec<u32>| {
            stack.extend(fields.iter().map(|field| field.ty.id));
        };
        match &ty.type_def {
            TypeDef::Composite(composite) => fields(&composite.fields, &mut stack),
            TypeDef::Variant(variants) => {
                for variant in &variants.variants {
                    fields(&variant.fields, &mut stack);
                }
            }
            TypeDef::Sequence(sequence) => stack.push(sequence.type_param.id),
            TypeDef::Array(array) => stack.push(array.type_param.id),
            TypeDef::Tuple(tuple) => stack.extend(tuple.fields.iter().map(|field| field.id)),
            TypeDef::Compact(compact) => stack.push(compact.type_param.id),
            TypeDef::Primitive(_) | TypeDef::BitSequence(_) => {}
        }
    }
    Ok(false)
}

fn signing_ready(runtime: &RuntimeHandle) -> Result<(), ComposeError> {
    if runtime.metadata_level() != MetadataLevel::V15 {
        return Err(ComposeError::MetadataDegraded);
    }
    runtime
        .signing_status()
        .map_err(ComposeError::SigningDisabled)
}

struct Plan {
    /// Live pallet and call indices.
    indices: [u8; 2],
    /// `(live field name, supplied position, layout)` in live order.
    fields: Vec<(String, usize, Layout)>,
}

fn plan(
    runtime: &RuntimeHandle,
    pallet: &str,
    call: &str,
    supplied: &[(&str, &ArgClass)],
) -> Result<Plan, ComposeError> {
    signing_ready(runtime)?;
    let core = runtime.core();
    let refuse = |field: Option<&str>, detail: String| incompatible(pallet, call, field, detail);
    let info = core
        .pallet(pallet)
        .ok_or_else(|| refuse(None, "pallet is absent".into()))?;
    let calls_type = info
        .calls_type
        .ok_or_else(|| refuse(None, "pallet has no calls".into()))?;
    let TypeDef::Variant(calls) = &resolve(core, calls_type)
        .map_err(|detail| refuse(None, detail))?
        .type_def
    else {
        return Err(refuse(None, "pallet call type is not an enum".into()));
    };
    let variant = calls
        .variants
        .iter()
        .find(|variant| variant.name == call)
        .ok_or_else(|| refuse(None, "call is absent".into()))?;
    let mut names = BTreeSet::new();
    for (name, _) in supplied {
        if !names.insert(*name) {
            return Err(refuse(Some(name), "argument is supplied twice".into()));
        }
    }
    let mut fields = Vec::with_capacity(variant.fields.len());
    for field in &variant.fields {
        let Some(name) = field.name.as_deref() else {
            return Err(refuse(None, "live call has an unnamed field".into()));
        };
        let Some(position) = supplied.iter().position(|(supplied, _)| *supplied == name) else {
            return Err(refuse(
                Some(name),
                "live field has no supplied argument (added or renamed)".into(),
            ));
        };
        let layout = layout(core, field.ty.id, supplied[position].1, 0)
            .map_err(|detail| refuse(Some(name), format!("live type {detail}")))?;
        fields.push((name.to_owned(), position, layout));
    }
    if let Some((name, _)) = supplied.iter().find(|(name, _)| {
        variant
            .fields
            .iter()
            .all(|f| f.name.as_deref() != Some(name))
    }) {
        return Err(refuse(
            Some(name),
            "argument is absent from the live call (removed or renamed)".into(),
        ));
    }
    Ok(Plan {
        indices: [info.index, variant.index],
        fields,
    })
}

/// Check that `pallet.call` can be composed by name from arguments of these
/// classes under `runtime`, without supplying values.
///
/// This is the readiness form of [`compose_resolved`]: the same metadata,
/// signing and per-field structural checks, and no encoding.
pub fn check_resolved(
    runtime: &RuntimeHandle,
    pallet: &str,
    call: &str,
    args: &[(&str, ArgClass)],
) -> Result<(), ComposeError> {
    let supplied = args
        .iter()
        .map(|(name, class)| (*name, class))
        .collect::<Vec<_>>();
    plan(runtime, pallet, call, &supplied).map(|_| ())
}

/// Compose `pallet.call` by name under `runtime` from arguments validated by
/// structural class.
///
/// The live field names must equal the supplied names exactly, in any order,
/// and each live type must fit its argument's class. The core encoder
/// produces the bytes; they are imported strictly, and each argument is
/// decoded back and compared with the supplied value before the bytes are
/// stamped with this runtime's versions.
pub fn compose_resolved(
    runtime: &RuntimeHandle,
    pallet: &str,
    call: &str,
    args: &[ResolvedArg<'_>],
) -> Result<CallBytes, ComposeError> {
    let refuse = |field: Option<&str>, detail: String| incompatible(pallet, call, field, detail);
    for arg in args {
        if !fits(&arg.class, &arg.value) {
            return Err(refuse(
                Some(arg.name),
                format!("supplied value is not {}", arg.class),
            ));
        }
    }
    let supplied = args
        .iter()
        .map(|arg| (arg.name, &arg.class))
        .collect::<Vec<_>>();
    let plan = plan(runtime, pallet, call, &supplied)?;
    let params = plan
        .fields
        .iter()
        .map(|(name, position, layout)| {
            core_value(layout, &args[*position].value)
                .map(|value| (name.clone(), value))
                .ok_or_else(|| refuse(Some(name), "value does not match its layout".into()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let encoded = runtime
        .core()
        .compose_call(pallet, call, &Value::record(params))
        .map_err(CoreError::from)?;
    let bytes = CallBytes::decode(&encoded, runtime)?;
    if bytes.as_bytes().get(..2) != Some(plan.indices.as_slice()) {
        return Err(refuse(
            None,
            "encoded call indices differ from the live call".into(),
        ));
    }
    let mut cursor = Cursor::new(&bytes.as_bytes()[2..]);
    cursor.strict = true;
    for (name, position, layout) in &plan.fields {
        let decoded = read(layout, &mut cursor)
            .map_err(|error| refuse(Some(name), format!("decode-back failed: {error}")))?;
        if decoded != args[*position].value {
            return Err(refuse(
                Some(name),
                "decoded value differs from the supplied value".into(),
            ));
        }
    }
    if cursor.remaining() != 0 {
        return Err(refuse(None, "encoded call has trailing bytes".into()));
    }
    Ok(bytes)
}

/// Read an imported call's pallet, call name and fields by name.
///
/// Fields named in `classes` must fit their class and are decoded by it;
/// every other field is decoded by the core and reported without a value.
/// `call` must carry this runtime's versions; the whole call must be
/// consumed. This never trusts a field it cannot type-check.
pub fn inspect_call(
    runtime: &RuntimeHandle,
    call: &CallBytes,
    classes: &[(&str, ArgClass)],
) -> Result<InspectedCall, ComposeError> {
    call.ensure_runtime(runtime)?;
    let core = runtime.core();
    let bytes = call.as_bytes();
    let malformed = || CoreError::Codec("call is not a pallet call".into());
    let [pallet_index, call_index, ..] = *bytes else {
        return Err(malformed().into());
    };
    let info = core.pallet_at(pallet_index).ok_or_else(malformed)?;
    let calls_type = info.calls_type.ok_or_else(malformed)?;
    let TypeDef::Variant(calls) = &core.resolve(calls_type).map_err(CoreError::from)?.type_def
    else {
        return Err(malformed().into());
    };
    let variant = calls
        .variants
        .iter()
        .find(|variant| variant.index == call_index)
        .ok_or_else(malformed)?;
    let (pallet, name) = (info.name.clone(), variant.name.clone());
    let refuse = |field: Option<&str>, detail: String| incompatible(&pallet, &name, field, detail);
    let mut cursor = Cursor::new(&bytes[2..]);
    cursor.strict = true;
    let mut fields = Vec::with_capacity(variant.fields.len());
    for field in &variant.fields {
        let field_name = field.name.clone().unwrap_or_default();
        let nests =
            nests_call(core, field.ty.id).map_err(|detail| refuse(Some(&field_name), detail))?;
        let value = match classes
            .iter()
            .find(|(class_name, _)| *class_name == field_name)
        {
            Some((_, class)) => {
                let layout = layout(core, field.ty.id, class, 0)
                    .map_err(|detail| refuse(Some(&field_name), format!("live type {detail}")))?;
                Some(read(&layout, &mut cursor)?)
            }
            None => {
                core.decode_id(field.ty.id, &mut cursor)
                    .map_err(CoreError::from)?;
                None
            }
        };
        fields.push(InspectedField {
            name: field_name,
            value,
            nests_call: nests,
        });
    }
    if cursor.remaining() != 0 {
        return Err(refuse(None, "call has trailing bytes".into()));
    }
    Ok(InspectedCall {
        pallet,
        call: name,
        fields,
    })
}
