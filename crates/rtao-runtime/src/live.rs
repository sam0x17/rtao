//! Typed calls composed from live metadata by name (D-123).
//!
//! A compiled [`crate::Call`] verifies its descriptor's exact argument shape
//! hash first. When that hash differs from the live runtime's, for example
//! because an argument's enum gained a variant (runtime 487 adds `Lock` and
//! `GammaSpend` to `ProxyType`), the call is composed from live metadata
//! instead of being refused:
//!
//! 1. the pallet and call are resolved by name in the live metadata;
//! 2. the supplied arguments must name exactly the live call's fields, so an
//!    added, removed or renamed field refuses with that field's name;
//! 3. every argument value must fit its live type without the core
//!    encoder's lenient coercions: named composites need exactly their
//!    fields, enum values name a live variant and supply exactly its fields,
//!    integers are never booleans, and so on;
//! 4. the one core encoder, `Runtime::compose_call`, encodes with live types
//!    and indices;
//! 5. the bytes are imported through the strict [`CallBytes::decode`], which
//!    decodes them completely under the live metadata and re-encodes them to
//!    the same bytes.
//!
//! Only a call that cannot be encoded this way refuses, per call, with the
//! reason. Each composition counts in [`live_compositions`] and logs one
//! warning per call and runtime.

use std::collections::BTreeSet;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use rtao_core::CoreError;
use rtao_core::codec::Value;
use rtao_core::runtime::Runtime;
use scale_info::{Field, TypeDef, TypeDefPrimitive, form::PortableForm};

use crate::handle::{CallBytes, ComposeError, MetadataLevel, RuntimeHandle, RuntimeStamp};

/// Deepest value nesting checked before encoding.
const MAX_DEPTH: usize = 128;
/// Distinct `(pallet, call, runtime)` warnings remembered per process.
const MAX_WARNED: usize = 1024;

static COMPOSITIONS: AtomicU64 = AtomicU64::new(0);
static WARNED: Mutex<BTreeSet<(String, String, u32, u32)>> = Mutex::new(BTreeSet::new());

/// Number of calls this process composed from live metadata because the
/// compiled descriptor no longer matched, or because the build has no
/// descriptor for the call (D-123). Applications export it as a metric.
pub fn live_compositions() -> u64 {
    COMPOSITIONS.load(Ordering::Relaxed)
}

/// Compose `pallet.call` from live metadata by name, without a compiled
/// descriptor (D-123).
///
/// `args` is a dictionary from live field name to a core value, the same
/// carrier generated builders produce: enum values by variant name, accounts
/// as 32 bytes or SS58 text, nested calls as already composed bytes. Every
/// live field must be supplied and no other key may appear. Each value must
/// fit its live type strictly (see the module documentation), and the
/// encoded call must import through [`CallBytes::decode`]. Nested call bytes
/// carry their own runtime stamp: compose them under this runtime first.
pub fn compose_by_name(
    runtime: &RuntimeHandle,
    pallet: &str,
    call: &str,
    args: &Value,
) -> Result<CallBytes, ComposeError> {
    let refuse = |detail: String| ComposeError::SurfaceChanged {
        pallet: pallet.into(),
        call: call.into(),
        detail,
    };
    if runtime.metadata_level() != MetadataLevel::V15 {
        return Err(ComposeError::MetadataDegraded);
    }
    runtime
        .signing_status()
        .map_err(ComposeError::SigningDisabled)?;
    let core = runtime.core();
    let fields = live_call_fields(core, pallet, call).map_err(refuse)?;
    fit_call_args(core, fields, args).map_err(refuse)?;
    let bytes = core
        .compose_call(pallet, call, args)
        .map_err(CoreError::from)?;
    let imported = CallBytes::decode(&bytes, runtime).map_err(|error| {
        refuse(format!(
            "strict import of the live encoding failed: {error}"
        ))
    })?;
    note(pallet, call, runtime.stamp(), None);
    Ok(imported)
}

/// Whether `descriptor`'s call exists in live metadata with exactly its
/// compiled field names, in any order: the precondition for composing it
/// from live metadata when its exact shape differs (D-123). Values are still
/// checked against the live types when the call is composed.
pub fn call_resolves_by_name(runtime: &RuntimeHandle, descriptor: &crate::CallDescriptor) -> bool {
    let Ok(fields) = live_call_fields(runtime.core(), descriptor.pallet, descriptor.name) else {
        return false;
    };
    fields.len() == descriptor.fields.len()
        && fields.iter().all(|field| {
            descriptor
                .fields
                .iter()
                .any(|compiled| field.name.as_deref() == Some(compiled.name))
        })
}

/// Whether field `index` of `descriptor`'s live event carries the compiled
/// type identity (the type path's last segment, or the primitive's name):
/// a positional read of that field stays correct when other fields of the
/// event changed (D-123).
pub fn event_field_matches(
    runtime: &RuntimeHandle,
    descriptor: &crate::EventDescriptor,
    index: usize,
) -> bool {
    let core = runtime.core();
    let live = (|| {
        let events = core.pallet(descriptor.pallet)?.events_type?;
        let TypeDef::Variant(variants) = &core.resolve(events).ok()?.type_def else {
            return None;
        };
        let event = variants
            .variants
            .iter()
            .find(|event| event.name == descriptor.name)?;
        let field = core.resolve(event.fields.get(index)?.ty.id).ok()?;
        Some(match &field.type_def {
            TypeDef::Primitive(primitive) => format!("{primitive:?}").to_lowercase(),
            _ => field.path.segments.last()?.clone(),
        })
    })();
    match (live, descriptor.fields.get(index)) {
        (Some(live), Some(compiled)) => live == compiled.type_ident,
        _ => false,
    }
}

/// Record a live composition and warn once per call and runtime.
pub(crate) fn note(pallet: &str, call: &str, stamp: RuntimeStamp, compiled: Option<&str>) {
    COMPOSITIONS.fetch_add(1, Ordering::Relaxed);
    let key = (
        pallet.to_owned(),
        call.to_owned(),
        stamp.spec_version,
        stamp.transaction_version,
    );
    let first = WARNED.lock().map_or(true, |mut warned| {
        if warned.len() >= MAX_WARNED && !warned.contains(&key) {
            warned.clear();
        }
        warned.insert(key)
    });
    if first {
        match compiled {
            Some(detail) => tracing::warn!(
                pallet,
                call,
                spec_version = stamp.spec_version,
                detail,
                "the compiled call no longer matches live metadata; composing it from live metadata by name"
            ),
            None => tracing::warn!(
                pallet,
                call,
                spec_version = stamp.spec_version,
                "this build has no descriptor for the call; composing it from live metadata by name"
            ),
        }
    }
}

fn live_call_fields<'a>(
    core: &'a Runtime,
    pallet: &str,
    call: &str,
) -> Result<&'a [Field<PortableForm>], String> {
    let info = core
        .pallet(pallet)
        .ok_or_else(|| format!("pallet {pallet} is absent from live metadata"))?;
    let calls = info
        .calls_type
        .ok_or_else(|| format!("pallet {pallet} has no calls in live metadata"))?;
    let ty = core.resolve(calls).map_err(|error| error.to_string())?;
    let TypeDef::Variant(variants) = &ty.type_def else {
        return Err(format!("pallet {pallet} call type is not an enum"));
    };
    variants
        .variants
        .iter()
        .find(|variant| variant.name == call)
        .map(|variant| variant.fields.as_slice())
        .ok_or_else(|| format!("call {pallet}.{call} is absent from live metadata"))
}

/// Check call arguments against the live call's fields by name.
pub(crate) fn fit_call_args(
    core: &Runtime,
    fields: &[Field<PortableForm>],
    args: &Value,
) -> Result<(), String> {
    if fields.iter().any(|field| field.name.is_none()) {
        return Err("the live call has unnamed fields, which cannot be matched by name".into());
    }
    let Value::Dict(entries) = args else {
        return Err("call arguments are not a name-to-value dictionary".into());
    };
    let supplied = dict_names(entries)?;
    for field in fields {
        let name = field.name.as_deref().unwrap_or_default();
        if !supplied.contains(name) {
            return Err(format!(
                "the live call has field {name}, which this call does not supply"
            ));
        }
    }
    for name in &supplied {
        if !fields
            .iter()
            .any(|field| field.name.as_deref() == Some(*name))
        {
            return Err(format!("argument {name} is not a field of the live call"));
        }
    }
    for field in fields {
        let name = field.name.as_deref().unwrap_or_default();
        let value = entry(entries, name).unwrap_or(&Value::Null);
        Fit { core, depth: 0 }
            .value(field.ty.id, value)
            .map_err(|detail| format!("field {name}: {detail}"))?;
    }
    Ok(())
}

fn dict_names(entries: &[(Value, Value)]) -> Result<BTreeSet<&str>, String> {
    let mut names = BTreeSet::new();
    for (key, _) in entries {
        let Value::Str(name) = key else {
            return Err("a dictionary key is not a field name".into());
        };
        if !names.insert(name.as_str()) {
            return Err(format!("field {name} is supplied twice"));
        }
    }
    Ok(names)
}

fn entry<'a>(entries: &'a [(Value, Value)], name: &str) -> Option<&'a Value> {
    entries
        .iter()
        .find(|(key, _)| matches!(key, Value::Str(key) if key == name))
        .map(|(_, value)| value)
}

fn is_empty(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::List(items) | Value::Tuple(items) => items.is_empty(),
        Value::Dict(entries) => entries.is_empty(),
        _ => false,
    }
}

fn kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Int(_) | Value::Uint(_) | Value::U256(_) => "an integer",
        Value::Str(_) => "a string",
        Value::Bytes(_) => "bytes",
        Value::List(_) | Value::Tuple(_) => "a list",
        Value::Dict(_) => "a dictionary",
    }
}

/// Strict structural check of one value against one live type.
struct Fit<'a> {
    core: &'a Runtime,
    depth: usize,
}

impl Fit<'_> {
    fn value(&mut self, id: u32, value: &Value) -> Result<(), String> {
        if self.depth >= MAX_DEPTH {
            return Err("value nesting exceeds 128 levels".into());
        }
        self.depth += 1;
        let result = self.inner(id, value);
        self.depth -= 1;
        result
    }

    fn inner(&mut self, id: u32, value: &Value) -> Result<(), String> {
        let core = self.core;
        let ty = core.resolve(id).map_err(|error| error.to_string())?;
        let segments = &ty.path.segments;
        let last = segments.last().map(String::as_str);
        let mismatch = |expected: &str| format!("expected {expected}, got {}", kind(value));
        match last {
            Some("AccountId32") => {
                return match value {
                    Value::Str(_) | Value::Bytes(_) => Ok(()),
                    _ => Err(mismatch("an account")),
                };
            }
            Some("Era") if segments.first().map(String::as_str) == Some("sp_runtime") => {
                return Ok(());
            }
            Some("MultiAddress") => {
                return match value {
                    Value::Str(_) | Value::Bytes(_) => Ok(()),
                    Value::Dict(_) => self.variant(id, &ty.type_def, value),
                    _ => Err(mismatch("an address")),
                };
            }
            Some("BTreeMap") => return self.map(&ty.type_def, value),
            _ => {}
        }
        if core.extrinsic.call_type == Some(id) && matches!(value, Value::Bytes(_)) {
            // Composed call bytes carry their own stamp, checked by the caller
            // and by the strict import of the whole call.
            return Ok(());
        }
        match &ty.type_def {
            TypeDef::Primitive(primitive) => primitive_fits(primitive, value),
            TypeDef::Compact(_) => match value {
                Value::Int(_) | Value::Uint(_) => Ok(()),
                _ => Err(mismatch("an integer")),
            },
            TypeDef::Composite(composite) => self.fields(&composite.fields, value),
            TypeDef::Variant(variants) => {
                if last == Some("Option") && segments.len() == 1 {
                    if matches!(value, Value::Null) {
                        return Ok(());
                    }
                    let some = variants
                        .variants
                        .iter()
                        .find(|variant| variant.name == "Some")
                        .ok_or("Option without Some")?;
                    return self.fields(&some.fields, value);
                }
                self.variant(id, &ty.type_def, value)
            }
            TypeDef::Sequence(sequence) => {
                if is_u8(core, sequence.type_param.id) {
                    return self.bytes(value, None);
                }
                let items = list(value).ok_or_else(|| mismatch("a list"))?;
                items
                    .iter()
                    .try_for_each(|item| self.value(sequence.type_param.id, item))
            }
            TypeDef::Array(array) => {
                let len = usize::try_from(array.len).map_err(|_| "array too long")?;
                if len > 0 && is_u8(core, array.type_param.id) {
                    return self.bytes(value, Some(len));
                }
                let items = list(value).ok_or_else(|| mismatch("a list"))?;
                if items.len() != len {
                    return Err(format!(
                        "expected {len} array elements, got {}",
                        items.len()
                    ));
                }
                items
                    .iter()
                    .try_for_each(|item| self.value(array.type_param.id, item))
            }
            TypeDef::Tuple(tuple) => {
                if tuple.fields.is_empty() {
                    return if is_empty(value) {
                        Ok(())
                    } else {
                        Err(mismatch("an empty tuple"))
                    };
                }
                let items = list(value).ok_or_else(|| mismatch("a tuple"))?;
                if items.len() != tuple.fields.len() {
                    return Err(format!(
                        "expected {} tuple elements, got {}",
                        tuple.fields.len(),
                        items.len()
                    ));
                }
                tuple
                    .fields
                    .iter()
                    .zip(items)
                    .try_for_each(|(field, item)| self.value(field.id, item))
            }
            TypeDef::BitSequence(_) => Err("bit sequences are not supported".into()),
        }
    }

    fn fields(&mut self, fields: &[Field<PortableForm>], value: &Value) -> Result<(), String> {
        if fields.is_empty() {
            return if is_empty(value) {
                Ok(())
            } else {
                Err(format!("expected no fields, got {}", kind(value)))
            };
        }
        if fields.iter().all(|field| field.name.is_some()) {
            let Value::Dict(entries) = value else {
                return Err(format!(
                    "expected a dictionary of fields, got {}",
                    kind(value)
                ));
            };
            let supplied = dict_names(entries)?;
            for field in fields {
                let name = field.name.as_deref().unwrap_or_default();
                if !supplied.contains(name) {
                    return Err(format!("live field {name} is not supplied"));
                }
            }
            if let Some(extra) = supplied.iter().find(|name| {
                !fields
                    .iter()
                    .any(|field| field.name.as_deref() == Some(**name))
            }) {
                return Err(format!("{extra} is not a live field"));
            }
            for field in fields {
                let name = field.name.as_deref().unwrap_or_default();
                let item = entry(entries, name).unwrap_or(&Value::Null);
                self.value(field.ty.id, item)
                    .map_err(|detail| format!("{name}: {detail}"))?;
            }
            return Ok(());
        }
        if let [field] = fields {
            // The core encoder unwraps one legacy list level around a
            // sequence in a one-field newtype; accept exactly that shape.
            if let Value::List(items) | Value::Tuple(items) = value
                && let [inner @ Value::List(_)] = items.as_slice()
                && matches!(
                    self.core.resolve(field.ty.id).map(|ty| &ty.type_def),
                    Ok(TypeDef::Sequence(_))
                )
            {
                return self.value(field.ty.id, inner);
            }
            return self.value(field.ty.id, value);
        }
        let items =
            list(value).ok_or_else(|| format!("expected a list of fields, got {}", kind(value)))?;
        if items.len() != fields.len() {
            return Err(format!(
                "expected {} fields, got {}",
                fields.len(),
                items.len()
            ));
        }
        fields
            .iter()
            .zip(items)
            .try_for_each(|(field, item)| self.value(field.ty.id, item))
    }

    fn variant(
        &mut self,
        id: u32,
        def: &TypeDef<PortableForm>,
        value: &Value,
    ) -> Result<(), String> {
        let TypeDef::Variant(variants) = def else {
            return Err(format!("type {id} is not an enum"));
        };
        let (name, payload) = match value {
            Value::Str(name) => (name.as_str(), None),
            Value::Dict(entries) => match entries.as_slice() {
                [(Value::Str(name), payload)] => (name.as_str(), Some(payload)),
                _ => return Err("an enum value must name exactly one variant".into()),
            },
            _ => return Err(format!("expected an enum variant, got {}", kind(value))),
        };
        let variant = variants
            .variants
            .iter()
            .find(|variant| variant.name == name)
            .ok_or_else(|| format!("live enum has no variant {name}"))?;
        match payload {
            None if variant.fields.is_empty() => Ok(()),
            None => Err(format!("variant {name} has fields that are not supplied")),
            Some(payload) => self
                .fields(&variant.fields, payload)
                .map_err(|detail| format!("{name}: {detail}")),
        }
    }

    fn map(&mut self, def: &TypeDef<PortableForm>, value: &Value) -> Result<(), String> {
        let core = self.core;
        let pair = (|| {
            let TypeDef::Composite(composite) = def else {
                return None;
            };
            let inner = composite.fields.first()?.ty.id;
            let TypeDef::Sequence(sequence) = &core.resolve(inner).ok()?.type_def else {
                return None;
            };
            let TypeDef::Tuple(pair) = &core.resolve(sequence.type_param.id).ok()?.type_def else {
                return None;
            };
            match pair.fields.as_slice() {
                [key, value] => Some((key.id, value.id)),
                _ => None,
            }
        })()
        .ok_or("BTreeMap layout is not a sequence of pairs")?;
        let Value::Dict(entries) = value else {
            return Err(format!("expected a map, got {}", kind(value)));
        };
        entries.iter().try_for_each(|(key, item)| {
            self.value(pair.0, key)?;
            self.value(pair.1, item)
        })
    }

    fn bytes(&mut self, value: &Value, len: Option<usize>) -> Result<(), String> {
        let count = match value {
            Value::Bytes(bytes) => bytes.len(),
            Value::Str(text) => match text.strip_prefix("0x") {
                Some(hex) => hex.len() / 2,
                None => text.len(),
            },
            Value::List(items) | Value::Tuple(items) => {
                for item in items {
                    match item {
                        Value::Int(byte) if (0..=255).contains(byte) => {}
                        Value::Uint(byte) if *byte <= 255 => {}
                        _ => return Err("a byte list element is not a byte".into()),
                    }
                }
                items.len()
            }
            _ => return Err(format!("expected bytes, got {}", kind(value))),
        };
        match len {
            Some(len) if len != count => Err(format!("expected {len} bytes, got {count}")),
            _ => Ok(()),
        }
    }
}

fn list(value: &Value) -> Option<&[Value]> {
    match value {
        Value::List(items) | Value::Tuple(items) => Some(items),
        _ => None,
    }
}

fn is_u8(core: &Runtime, id: u32) -> bool {
    matches!(
        core.resolve(id).map(|ty| &ty.type_def),
        Ok(TypeDef::Primitive(TypeDefPrimitive::U8))
    )
}

fn primitive_fits(primitive: &TypeDefPrimitive, value: &Value) -> Result<(), String> {
    use TypeDefPrimitive as P;
    let ok = match primitive {
        P::Bool => matches!(value, Value::Bool(_)),
        P::Char | P::Str => matches!(value, Value::Str(_)),
        P::U256 | P::I256 => matches!(value, Value::Int(_) | Value::Uint(_) | Value::U256(_)),
        _ => matches!(value, Value::Int(_) | Value::Uint(_)),
    };
    if ok {
        Ok(())
    } else {
        Err(format!("expected {primitive:?}, got {}", kind(value)))
    }
}
