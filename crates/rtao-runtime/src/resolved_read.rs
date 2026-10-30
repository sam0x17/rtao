//! Name-resolved storage and constant reads validated by structural class (D-089).
//!
//! A typed read refuses whenever the live key/value shape hash differs from
//! the build's, even when the change cannot alter the value the reader
//! needs: a widened or compact integer, a single-field newtype, `Vec`
//! replaced by `BoundedVec` or `VecDeque`, a tuple turned into a struct,
//! reordered or added struct fields, or an added enum variant. This module
//! resolves an entry by name against live metadata and decodes exactly those
//! changes into a [`ReadDatum`]. Each key and the value declare a structural
//! [`ReadClass`]; the live type must fit it once single-field composites and
//! `Compact` are removed.
//!
//! Anything whose meaning cannot be recovered refuses with a
//! [`ReadResolveError`] naming the entry, the key or value position and the
//! field: a signed integer, an `Option`, a struct where a scalar was read, a
//! removed field, a renamed field without a declared alias, a changed tuple
//! arity or key count, a removed or payload-carrying declared variant, and a
//! missing-value modifier that would turn an absent entry into a default.
//! A named struct is never decoded by position. Decoding is strict: every
//! byte must be consumed, an integer must fit both its live type and the
//! class's width, and an unknown variant index refuses.

use std::fmt;

use parity_scale_codec::{Compact, Encode};
use rtao_core::codec::decode::{Cursor, compact_len, compact_u128};
use rtao_core::codec::storage::{hash_param, storage_prefix};
use rtao_core::runtime::{Runtime, StorageInfo};
use scale_info::{Field, Type, TypeDef, TypeDefPrimitive, form::PortableForm};

use crate::handle::RuntimeHandle;

/// Deepest chain of layers followed for one key or value.
const MAX_LAYERS: usize = 16;

/// Structural class one name-resolved key or value must fit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadClass {
    /// An unsigned integer, fixed width or compact, whose values must fit
    /// this many bits. A live type of any unsigned width fits; each decoded
    /// value is range checked.
    Uint(u8),
    /// A boolean.
    Bool,
    /// A 32-byte account identifier (`[u8; 32]` through newtypes).
    Account,
    /// A length-prefixed byte sequence (`Vec<u8>`, `BoundedVec<u8, _>`).
    Bytes,
    /// A sequence or fixed array whose elements fit the inner class.
    Seq(&'static ReadClass),
    /// A tuple, unnamed struct or named struct.
    ///
    /// A tuple or unnamed struct must have exactly these fields, in order. A
    /// named struct's fields are matched by [`ReadField::names`] in any
    /// order; live fields that match no declared field are skipped.
    Record(&'static [ReadField]),
    /// An enum whose listed unit variants must exist by name. Other live
    /// variants are allowed and decode to their name.
    Variants(&'static [&'static str]),
}

/// One declared field of a [`ReadClass::Record`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReadField {
    /// Accepted live names, preferred name first. A live named struct must
    /// have exactly one field with one of these names.
    pub names: &'static [&'static str],
    /// Structural class of the field.
    pub class: ReadClass,
}

impl fmt::Display for ReadClass {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Uint(bits) => write!(formatter, "an unsigned integer (read as u{bits})"),
            Self::Bool => formatter.write_str("a bool"),
            Self::Account => formatter.write_str("a 32-byte account"),
            Self::Bytes => formatter.write_str("a byte sequence"),
            Self::Seq(inner) => write!(formatter, "a sequence of {inner}"),
            Self::Record(fields) => write!(formatter, "a record of {} fields", fields.len()),
            Self::Variants(names) => write!(formatter, "an enum with {}", names.join(", ")),
        }
    }
}

/// A value decoded by a [`ReadClass`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReadDatum {
    /// An unsigned integer within its class's width.
    Uint(u128),
    /// A boolean.
    Bool(bool),
    /// An account identifier.
    Account([u8; 32]),
    /// Raw bytes of a byte sequence.
    Bytes(Vec<u8>),
    /// Elements of a sequence.
    Seq(Vec<ReadDatum>),
    /// Record fields in declared order, whatever the live order.
    Record(Vec<ReadDatum>),
    /// The live name of an enum variant.
    Variant(String),
}

impl ReadDatum {
    /// The integer, when this is one.
    pub fn as_uint(&self) -> Option<u128> {
        match self {
            Self::Uint(value) => Some(*value),
            _ => None,
        }
    }
    /// The boolean, when this is one.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(value) => Some(*value),
            _ => None,
        }
    }
    /// The account bytes, when this is an account.
    pub fn as_account(&self) -> Option<[u8; 32]> {
        match self {
            Self::Account(value) => Some(*value),
            _ => None,
        }
    }
    /// The variant name, when this is an enum value.
    pub fn as_variant(&self) -> Option<&str> {
        match self {
            Self::Variant(name) => Some(name),
            _ => None,
        }
    }
}

/// How an absent storage entry reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadPresence {
    /// A live `Default` entry reads its live default bytes when absent. An
    /// absent entry of a live `Optional` entry refuses that read.
    Default,
    /// Absence is meaningful (`None`); the live entry must stay `Optional`,
    /// since a `Default` entry would read an absent key as its default.
    Optional,
}

/// A storage entry read by name and structural class.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolvedStorage {
    /// Pallet name.
    pub pallet: &'static str,
    /// Entry name.
    pub name: &'static str,
    /// Classes of the unhashed keys, in order; the live key count must match.
    pub keys: &'static [ReadClass],
    /// Class of the value.
    pub value: ReadClass,
    /// How an absent entry reads.
    pub presence: ReadPresence,
}

/// A name-resolved read does not fit its class under the live runtime.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadResolveError {
    /// Pallet of the entry or constant.
    pub pallet: String,
    /// Entry or constant name.
    pub item: String,
    /// Refusing position, such as `key 1`, `value` or `modifier`.
    pub part: String,
    /// Exact reason, naming the field or element when there is one.
    pub detail: String,
}

impl fmt::Display for ReadResolveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}.{} {}: {}",
            self.pallet, self.item, self.part, self.detail
        )
    }
}

impl std::error::Error for ReadResolveError {}

/// How one live type carries a class.
#[derive(Clone, Debug)]
enum Layout {
    Uint {
        bits: u8,
        compact: bool,
        read: u8,
    },
    Bool,
    Account,
    Bytes,
    Seq(Box<Layout>),
    Array(usize, Box<Layout>),
    /// Live fields in live order, and the declared field count.
    Record(Vec<Slot>, usize),
    /// Every live variant as `(index, name, payload field types)`.
    Variants(Vec<(u8, String, Vec<u32>)>),
}

#[derive(Clone, Debug)]
enum Slot {
    /// Decode into this declared position.
    Take(usize, Layout),
    /// An undeclared live field, decoded by the core and discarded.
    Skip(u32),
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

fn is_u8(core: &Runtime, id: u32) -> Result<bool, String> {
    Ok(matches!(
        resolve(core, id)?.type_def,
        TypeDef::Primitive(TypeDefPrimitive::U8)
    ))
}

/// Width of a compact integer's payload, through single-field composites.
fn compact_bits(core: &Runtime, mut id: u32) -> Result<u8, String> {
    for _ in 0..MAX_LAYERS {
        let ty = resolve(core, id)?;
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

/// Whether a single-field struct is the record itself rather than a newtype
/// around it: its one field carries a declared name, or the record declares
/// one positional field.
fn single_field_record(field: &Field<PortableForm>, fields: &[ReadField]) -> bool {
    match field.name.as_deref() {
        Some(name) => fields.iter().any(|declared| declared.names.contains(&name)),
        None => fields.len() == 1,
    }
}

fn positional(
    core: &Runtime,
    types: &[u32],
    fields: &[ReadField],
    depth: usize,
    what: &str,
) -> Result<Layout, String> {
    if types.len() != fields.len() {
        return Err(format!(
            "is {what} of {} fields, expected {}",
            types.len(),
            fields.len()
        ));
    }
    let slots = types
        .iter()
        .zip(fields)
        .enumerate()
        .map(|(position, (ty, field))| {
            layout(core, *ty, &field.class, depth + 1)
                .map(|layout| Slot::Take(position, layout))
                .map_err(|detail| format!("field {position} ({}) {detail}", field.names[0]))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Layout::Record(slots, fields.len()))
}

fn named(
    core: &Runtime,
    live: &[Field<PortableForm>],
    fields: &[ReadField],
    depth: usize,
) -> Result<Layout, String> {
    let mut matched = vec![None::<&str>; fields.len()];
    let mut slots = Vec::with_capacity(live.len());
    for field in live {
        let name = field.name.as_deref().unwrap_or_default();
        let Some(position) = fields
            .iter()
            .position(|declared| declared.names.contains(&name))
        else {
            slots.push(Slot::Skip(field.ty.id));
            continue;
        };
        if let Some(previous) = matched[position] {
            return Err(format!(
                "fields `{previous}` and `{name}` both match `{}`",
                fields[position].names[0]
            ));
        }
        matched[position] = Some(name);
        let layout = layout(core, field.ty.id, &fields[position].class, depth + 1)
            .map_err(|detail| format!("field `{name}` {detail}"))?;
        slots.push(Slot::Take(position, layout));
    }
    if let Some(position) = matched.iter().position(Option::is_none) {
        return Err(format!(
            "field `{}` is absent from the live struct (removed, or renamed without a declared alias)",
            fields[position].names[0]
        ));
    }
    Ok(Layout::Record(slots, fields.len()))
}

fn layout(core: &Runtime, id: u32, class: &ReadClass, depth: usize) -> Result<Layout, String> {
    if depth >= MAX_LAYERS {
        return Err(format!("nests more than {MAX_LAYERS} layers"));
    }
    let ty = resolve(core, id)?;
    let mismatch = || format!("is {}, expected {class}", describe(ty));
    if let TypeDef::Composite(composite) = &ty.type_def
        && let [field] = composite.fields.as_slice()
        && !matches!(class, ReadClass::Record(fields) if single_field_record(field, fields))
    {
        return layout(core, field.ty.id, class, depth + 1);
    }
    match (&ty.type_def, class) {
        (TypeDef::Primitive(primitive), ReadClass::Uint(read)) => unsigned_bits(primitive)
            .map(|bits| Layout::Uint {
                bits,
                compact: false,
                read: *read,
            })
            .ok_or_else(mismatch),
        (TypeDef::Compact(compact), ReadClass::Uint(read)) => Ok(Layout::Uint {
            bits: compact_bits(core, compact.type_param.id)?,
            compact: true,
            read: *read,
        }),
        (TypeDef::Primitive(TypeDefPrimitive::Bool), ReadClass::Bool) => Ok(Layout::Bool),
        (TypeDef::Array(array), ReadClass::Account)
            if array.len == 32 && is_u8(core, array.type_param.id)? =>
        {
            Ok(Layout::Account)
        }
        (TypeDef::Sequence(sequence), ReadClass::Bytes) if is_u8(core, sequence.type_param.id)? => {
            Ok(Layout::Bytes)
        }
        (TypeDef::Sequence(sequence), ReadClass::Seq(inner)) => Ok(Layout::Seq(Box::new(
            layout(core, sequence.type_param.id, inner, depth + 1)
                .map_err(|detail| format!("element {detail}"))?,
        ))),
        (TypeDef::Array(array), ReadClass::Seq(inner)) => Ok(Layout::Array(
            array.len as usize,
            Box::new(
                layout(core, array.type_param.id, inner, depth + 1)
                    .map_err(|detail| format!("element {detail}"))?,
            ),
        )),
        (TypeDef::Tuple(tuple), ReadClass::Record(fields)) => {
            let types = tuple
                .fields
                .iter()
                .map(|field| field.id)
                .collect::<Vec<_>>();
            positional(core, &types, fields, depth, "a tuple")
        }
        (TypeDef::Composite(composite), ReadClass::Record(fields))
            if !composite.fields.is_empty()
                && composite.fields.iter().all(|field| field.name.is_none()) =>
        {
            let types = composite
                .fields
                .iter()
                .map(|field| field.ty.id)
                .collect::<Vec<_>>();
            positional(core, &types, fields, depth, "an unnamed struct")
        }
        (TypeDef::Composite(composite), ReadClass::Record(fields))
            if !composite.fields.is_empty()
                && composite.fields.iter().all(|field| field.name.is_some()) =>
        {
            named(core, &composite.fields, fields, depth)
        }
        (TypeDef::Variant(variants), ReadClass::Variants(names))
            if ty.path.segments.last().is_none_or(|last| last != "Option") =>
        {
            for name in *names {
                let Some(variant) = variants.variants.iter().find(|v| v.name == *name) else {
                    return Err(format!(
                        "variant `{name}` is absent from the live enum (removed or renamed)"
                    ));
                };
                if !variant.fields.is_empty() {
                    return Err(format!("variant `{name}` now carries fields"));
                }
            }
            Ok(Layout::Variants(
                variants
                    .variants
                    .iter()
                    .map(|variant| {
                        (
                            variant.index,
                            variant.name.clone(),
                            variant.fields.iter().map(|field| field.ty.id).collect(),
                        )
                    })
                    .collect(),
            ))
        }
        _ => Err(mismatch()),
    }
}

fn codec(error: impl fmt::Display) -> String {
    error.to_string()
}

fn read(core: &Runtime, layout: &Layout, cursor: &mut Cursor<'_>) -> Result<ReadDatum, String> {
    match layout {
        Layout::Uint {
            bits,
            compact,
            read,
        } => {
            let value = if *compact {
                compact_u128(cursor).map_err(codec)?
            } else {
                let bytes = cursor.take(usize::from(*bits / 8)).map_err(codec)?;
                let mut buf = [0u8; 16];
                buf[..bytes.len()].copy_from_slice(bytes);
                u128::from_le_bytes(buf)
            };
            if *bits < 128 && value >> u32::from(*bits) != 0 {
                return Err(format!(
                    "compact value {value} exceeds its live u{bits} type"
                ));
            }
            if *read < 128 && value >> u32::from(*read) != 0 {
                return Err(format!(
                    "value {value} does not fit the u{read} this read uses"
                ));
            }
            Ok(ReadDatum::Uint(value))
        }
        Layout::Bool => match cursor.byte().map_err(codec)? {
            0 => Ok(ReadDatum::Bool(false)),
            1 => Ok(ReadDatum::Bool(true)),
            other => Err(format!("invalid bool byte {other}")),
        },
        Layout::Account => {
            let bytes = cursor.take(32).map_err(codec)?;
            let mut account = [0u8; 32];
            account.copy_from_slice(bytes);
            Ok(ReadDatum::Account(account))
        }
        Layout::Bytes => {
            let len = compact_len(cursor).map_err(codec)?;
            Ok(ReadDatum::Bytes(cursor.take(len).map_err(codec)?.to_vec()))
        }
        Layout::Seq(inner) => {
            let len = compact_len(cursor).map_err(codec)?;
            elements(core, inner, len, cursor)
        }
        Layout::Array(len, inner) => elements(core, inner, *len, cursor),
        Layout::Record(slots, count) => {
            let mut fields = vec![None; *count];
            for slot in slots {
                match slot {
                    Slot::Take(position, layout) => {
                        fields[*position] = Some(read(core, layout, cursor)?);
                    }
                    Slot::Skip(ty) => {
                        core.decode_id(*ty, cursor).map_err(codec)?;
                    }
                }
            }
            fields
                .into_iter()
                .collect::<Option<Vec<_>>>()
                .map(ReadDatum::Record)
                .ok_or_else(|| "record field was not decoded".to_owned())
        }
        Layout::Variants(variants) => {
            let index = cursor.byte().map_err(codec)?;
            let (_, name, fields) = variants
                .iter()
                .find(|(candidate, _, _)| *candidate == index)
                .ok_or_else(|| format!("unknown variant index {index}"))?;
            for ty in fields {
                core.decode_id(*ty, cursor).map_err(codec)?;
            }
            Ok(ReadDatum::Variant(name.clone()))
        }
    }
}

fn elements(
    core: &Runtime,
    inner: &Layout,
    len: usize,
    cursor: &mut Cursor<'_>,
) -> Result<ReadDatum, String> {
    // Every layout consumes at least one byte per element.
    if len > cursor.remaining() {
        return Err(format!(
            "sequence of {len} elements exceeds the remaining bytes"
        ));
    }
    (0..len)
        .map(|index| {
            read(core, inner, cursor).map_err(|detail| format!("element {index} {detail}"))
        })
        .collect::<Result<_, _>>()
        .map(ReadDatum::Seq)
}

fn decode_complete(core: &Runtime, layout: &Layout, bytes: &[u8]) -> Result<ReadDatum, String> {
    let mut cursor = Cursor::new(bytes);
    cursor.strict = true;
    let datum = read(core, layout, &mut cursor)?;
    match cursor.remaining() {
        0 => Ok(datum),
        trailing => Err(format!("{trailing} trailing bytes remain")),
    }
}

/// Encode a key component under its live layout. Keys are scalars,
/// accounts, byte strings and sequences of them.
fn encode_key(layout: &Layout, datum: &ReadDatum, out: &mut Vec<u8>) -> Result<(), String> {
    match (layout, datum) {
        (
            Layout::Uint {
                bits,
                compact,
                read,
            },
            ReadDatum::Uint(value),
        ) => {
            if *read < 128 && value >> u32::from(*read) != 0 {
                return Err(format!("key value {value} does not fit u{read}"));
            }
            if *bits < 128 && value >> u32::from(*bits) != 0 {
                return Err(format!("key value {value} does not fit the live u{bits}"));
            }
            if *compact {
                Compact(*value).encode_to(out);
            } else {
                out.extend_from_slice(&value.to_le_bytes()[..usize::from(*bits / 8)]);
            }
        }
        (Layout::Bool, ReadDatum::Bool(value)) => out.push(u8::from(*value)),
        (Layout::Account, ReadDatum::Account(account)) => out.extend_from_slice(account),
        (Layout::Bytes, ReadDatum::Bytes(bytes)) => {
            Compact(bytes.len() as u64).encode_to(out);
            out.extend_from_slice(bytes);
        }
        (Layout::Seq(inner), ReadDatum::Seq(items)) => {
            Compact(items.len() as u64).encode_to(out);
            for item in items {
                encode_key(inner, item, out)?;
            }
        }
        (Layout::Array(len, inner), ReadDatum::Seq(items)) if items.len() == *len => {
            for item in items {
                encode_key(inner, item, out)?;
            }
        }
        (Layout::Record(..) | Layout::Variants(_), _) => {
            return Err("records and enums are not supported as name-resolved keys".into());
        }
        _ => return Err("supplied key does not match its class".into()),
    }
    Ok(())
}

fn key_supported(layout: &Layout) -> bool {
    match layout {
        Layout::Uint { .. } | Layout::Bool | Layout::Account | Layout::Bytes => true,
        Layout::Seq(inner) | Layout::Array(_, inner) => key_supported(inner),
        Layout::Record(..) | Layout::Variants(_) => false,
    }
}

/// A [`ResolvedStorage`] entry checked against one runtime, ready to build
/// keys and decode values under it.
#[derive(Clone, Debug)]
pub struct ResolvedStoragePlan {
    storage: ResolvedStorage,
    runtime: RuntimeHandle,
    info: StorageInfo,
    keys: Vec<Layout>,
    value: Layout,
}

impl ResolvedStorage {
    fn refuse(&self, part: impl Into<String>, detail: impl Into<String>) -> ReadResolveError {
        ReadResolveError {
            pallet: self.pallet.into(),
            item: self.name.into(),
            part: part.into(),
            detail: detail.into(),
        }
    }

    /// Check only the live missing-value modifier against [`Self::presence`].
    ///
    /// A typed read's exact shape hash does not cover the modifier, so a
    /// compiled read of an entry whose absence is meaningful runs this check
    /// too.
    pub fn check_presence(&self, runtime: &RuntimeHandle) -> Result<(), ReadResolveError> {
        let info = runtime
            .storage_entry(self.pallet, self.name)
            .ok_or_else(|| self.refuse("entry", "storage entry is absent"))?;
        self.presence_fits(info)
    }

    fn presence_fits(&self, info: &StorageInfo) -> Result<(), ReadResolveError> {
        match (self.presence, info.modifier.as_str()) {
            (ReadPresence::Default, "Default" | "Optional")
            | (ReadPresence::Optional, "Optional") => Ok(()),
            (ReadPresence::Optional, "Default") => Err(self.refuse(
                "modifier",
                "the live entry is no longer Optional; an absent key would read as its default",
            )),
            (_, other) => Err(self.refuse(
                "modifier",
                format!("unknown missing-value modifier {other:?}"),
            )),
        }
    }

    /// Check that this entry can be read by name under `runtime`.
    pub fn check(&self, runtime: &RuntimeHandle) -> Result<(), ReadResolveError> {
        self.plan(runtime).map(|_| ())
    }

    /// Resolve this entry's keys and value against `runtime`.
    pub fn plan(&self, runtime: &RuntimeHandle) -> Result<ResolvedStoragePlan, ReadResolveError> {
        let core = runtime.core();
        let info = runtime
            .storage_entry(self.pallet, self.name)
            .ok_or_else(|| self.refuse("entry", "storage entry is absent"))?;
        if info.key_types.len() != self.keys.len() || info.hashers.len() != self.keys.len() {
            return Err(self.refuse(
                "keys",
                format!(
                    "the live entry has {} keys, expected {}",
                    info.key_types.len(),
                    self.keys.len()
                ),
            ));
        }
        let keys = info
            .key_types
            .iter()
            .zip(self.keys)
            .enumerate()
            .map(|(position, (ty, class))| {
                let part = || format!("key {position}");
                let layout = layout(core, *ty, class, 0)
                    .map_err(|detail| self.refuse(part(), format!("live type {detail}")))?;
                if key_supported(&layout) {
                    Ok(layout)
                } else {
                    Err(self.refuse(part(), "is not a scalar, account or byte key"))
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        let value = layout(core, info.value_type, &self.value, 0)
            .map_err(|detail| self.refuse("value", format!("live type {detail}")))?;
        self.presence_fits(info)?;
        if info.modifier == "Default" {
            decode_complete(core, &value, &info.default_bytes).map_err(|detail| {
                self.refuse("default", format!("live default does not decode: {detail}"))
            })?;
        }
        Ok(ResolvedStoragePlan {
            storage: *self,
            runtime: runtime.clone(),
            info: info.clone(),
            keys,
            value,
        })
    }
}

impl ResolvedStoragePlan {
    /// The entry this plan reads.
    pub fn storage(&self) -> &ResolvedStorage {
        &self.storage
    }

    /// The runtime this plan was resolved against.
    pub fn runtime(&self) -> &RuntimeHandle {
        &self.runtime
    }

    /// Build the full storage key from unhashed key values, encoded with the
    /// live key types, hashers and prefix.
    pub fn key(&self, keys: &[ReadDatum]) -> Result<Vec<u8>, ReadResolveError> {
        if keys.len() != self.keys.len() {
            return Err(self.storage.refuse(
                "keys",
                format!(
                    "{} key values supplied, expected {}",
                    keys.len(),
                    self.keys.len()
                ),
            ));
        }
        let mut key = storage_prefix(&self.info);
        for (position, ((layout, datum), hasher)) in self
            .keys
            .iter()
            .zip(keys)
            .zip(&self.info.hashers)
            .enumerate()
        {
            let mut encoded = Vec::new();
            encode_key(layout, datum, &mut encoded)
                .map_err(|detail| self.storage.refuse(format!("key {position}"), detail))?;
            let hashed = hash_param(hasher, &encoded)
                .map_err(|error| self.storage.refuse(format!("key {position}"), codec(error)))?;
            key.extend_from_slice(&hashed);
        }
        Ok(key)
    }

    /// Decode a raw value, or apply the live missing-value semantics when
    /// `raw` is absent. Returns `None` only for an absent
    /// [`ReadPresence::Optional`] entry.
    pub fn decode(&self, raw: Option<&[u8]>) -> Result<Option<ReadDatum>, ReadResolveError> {
        let core = self.runtime.core();
        let bytes = match (raw, self.info.modifier.as_str()) {
            (Some(bytes), _) => bytes,
            (None, "Default") => self.info.default_bytes.as_slice(),
            (None, _) if self.storage.presence == ReadPresence::Optional => return Ok(None),
            (None, _) => {
                return Err(self
                    .storage
                    .refuse("value", "entry is absent and the live entry has no default"));
            }
        };
        decode_complete(core, &self.value, bytes)
            .map(Some)
            .map_err(|detail| self.storage.refuse("value", detail))
    }
}

/// Check that constant `pallet.name` can be read by name as `class`.
pub fn check_constant(
    runtime: &RuntimeHandle,
    pallet: &str,
    name: &str,
    class: &ReadClass,
) -> Result<(), ReadResolveError> {
    read_constant(runtime, pallet, name, class).map(|_| ())
}

/// Read constant `pallet.name` by name, decoding its live value as `class`.
pub fn read_constant(
    runtime: &RuntimeHandle,
    pallet: &str,
    name: &str,
    class: &ReadClass,
) -> Result<ReadDatum, ReadResolveError> {
    let refuse = |part: &str, detail: String| ReadResolveError {
        pallet: pallet.into(),
        item: name.into(),
        part: part.into(),
        detail,
    };
    let core = runtime.core();
    let info = core
        .constant(pallet, name)
        .ok_or_else(|| refuse("constant", "constant is absent".into()))?;
    let layout = layout(core, info.ty, class, 0)
        .map_err(|detail| refuse("value", format!("live type {detail}")))?;
    decode_complete(core, &layout, &info.value).map_err(|detail| refuse("value", detail))
}

#[cfg(test)]
#[path = "resolved_read_tests.rs"]
mod tests;
