//! Bounds-checked borrowed parser for the exact pinned metagraph layout.

use std::collections::BTreeMap;
use std::ops::Range;

use rtao_core::codec::{
    Value,
    decode::{Cursor, compact_len, compact_u128},
};
use rtao_runtime::{RuntimeHandle, shapes};
use scale_info::TypeDef;

use crate::{MetagraphError, Result};

/// Fixed-width columns remain slices of the original response.
#[derive(Clone, Debug)]
pub(crate) struct BorrowedColumns {
    pub fixed: BTreeMap<String, (Range<usize>, usize)>,
    pub compact: BTreeMap<String, Vec<u64>>,
    pub other: BTreeMap<String, Value>,
}

impl BorrowedColumns {
    pub(crate) fn value(&self, raw: &[u8], name: &str, uid: usize) -> Option<Value> {
        if let Some(values) = self.compact.get(name) {
            return values.get(uid).map(|value| Value::Int(i128::from(*value)));
        }
        if let Some((range, width)) = self.fixed.get(name) {
            let offset = uid.checked_mul(*width)?.checked_add(range.start)?;
            let end = offset.checked_add(*width)?;
            if end > range.end {
                return None;
            }
            let bytes = raw.get(offset..end)?;
            return match name {
                "hotkeys" | "coldkeys" => {
                    let key: [u8; 32] = bytes.try_into().ok()?;
                    Some(Value::Str(rtao_core::keys::ss58_from_public(key, 42)))
                }
                "active" | "validator_permit" => bytes.first().map(|byte| Value::Bool(*byte != 0)),
                "axons" => axon(bytes).ok(),
                _ => None,
            };
        }
        match self.other.get(name)? {
            Value::List(values) => values.get(uid).cloned(),
            _ => None,
        }
    }

    pub(crate) fn dynamic(&self, raw: &[u8], name: &str) -> Value {
        if let Some(values) = self.compact.get(name) {
            return Value::List(
                values
                    .iter()
                    .map(|value| Value::Int(i128::from(*value)))
                    .collect(),
            );
        }
        if let Some((range, width)) = self.fixed.get(name) {
            return Value::List(
                (0..range.len() / width)
                    .filter_map(|uid| self.value(raw, name, uid))
                    .collect(),
            );
        }
        self.other.get(name).cloned().unwrap_or(Value::Null)
    }
}

/// Borrowed parser result, admitted only when the runtime's exact layout matches.
#[derive(Debug)]
pub struct MetagraphView<'a> {
    raw: &'a [u8],
    pub(crate) fields: Vec<(String, Value)>,
    pub(crate) columns: BorrowedColumns,
}

impl<'a> MetagraphView<'a> {
    /// Parse a complete `Option<Metagraph>` response against its live runtime.
    ///
    /// A shape mismatch is an error. Callers can then select a by-name or
    /// dynamic decoder. No unchecked slices or allocation from unbounded lengths
    /// are used by this parser.
    pub fn parse(raw: &'a [u8], runtime: &RuntimeHandle) -> Result<Option<Self>> {
        let method = runtime
            .runtime_api("SubnetInfoRuntimeApi", "get_metagraph")
            .ok_or_else(|| MetagraphError::Shape("get_metagraph is absent".into()))?;
        let option = runtime
            .core()
            .types
            .resolve(method.output)
            .ok_or_else(|| MetagraphError::Shape("output type is absent".into()))?;
        let TypeDef::Variant(variants) = &option.type_def else {
            return Err(MetagraphError::Shape("output is not Option".into()));
        };
        if option.path.segments.as_slice() != ["Option"]
            || variants.variants.len() != 2
            || !variants.variants.iter().any(|variant| {
                variant.name == "None" && variant.index == 0 && variant.fields.is_empty()
            })
        {
            return Err(MetagraphError::Shape(
                "output Option encoding changed".into(),
            ));
        }
        let some = variants
            .variants
            .iter()
            .find(|variant| variant.name == "Some")
            .ok_or_else(|| MetagraphError::Shape("output has no Some variant".into()))?;
        let field = some
            .fields
            .first()
            .ok_or_else(|| MetagraphError::Shape("Some has no payload".into()))?;
        if some.index != 1
            || some.fields.len() != 1
            || runtime.exact_shape(field.ty.id)? != shapes::metagraph::EXACT
        {
            return Err(MetagraphError::Shape(
                "metagraph exact layout changed".into(),
            ));
        }
        let mut cursor = Cursor::new(raw);
        let tag = cursor.byte()?;
        if tag == 0 {
            if cursor.remaining() != 0 {
                return Err(MetagraphError::TrailingBytes);
            }
            return Ok(None);
        }
        if tag != some.index {
            return Err(MetagraphError::Malformed(
                "invalid metagraph option tag".into(),
            ));
        }
        let ty = runtime
            .core()
            .types
            .resolve(field.ty.id)
            .ok_or_else(|| MetagraphError::Shape("metagraph type is absent".into()))?;
        let TypeDef::Composite(composite) = &ty.type_def else {
            return Err(MetagraphError::Shape("metagraph is not a record".into()));
        };
        let mut columns = BorrowedColumns {
            fixed: BTreeMap::new(),
            compact: BTreeMap::new(),
            other: BTreeMap::new(),
        };
        let mut fields = Vec::new();
        for field in &composite.fields {
            let name = field
                .name
                .as_deref()
                .ok_or_else(|| MetagraphError::Shape("unnamed metagraph field".into()))?;
            let width = match name {
                "hotkeys" | "coldkeys" => Some(32),
                "axons" => Some(34),
                "active" | "validator_permit" => Some(1),
                _ => None,
            };
            let compact_width = match name {
                "pruning_score" | "dividends" | "incentives" | "consensus" | "trust" | "rank" => {
                    Some(u64::from(u16::MAX))
                }
                "last_update"
                | "emission"
                | "block_at_registration"
                | "alpha_stake"
                | "tao_stake"
                | "total_stake"
                | "collateral_locked"
                | "collateral_min"
                | "collateral_earned" => Some(u64::MAX),
                _ => None,
            };
            if let Some(width) = width {
                let count = compact_len(&mut cursor)?;
                let size = count.checked_mul(width).ok_or_else(|| {
                    MetagraphError::Malformed("column byte length overflows".into())
                })?;
                let start = cursor.offset;
                let bytes = cursor.take(size)?;
                if width == 1 && bytes.iter().any(|byte| *byte > 1) {
                    return Err(MetagraphError::Malformed("invalid boolean column".into()));
                }
                columns
                    .fixed
                    .insert(name.into(), (start..cursor.offset, width));
                fields.push((name.into(), Value::Null));
            } else if let Some(max) = compact_width {
                let count = compact_len(&mut cursor)?;
                if count > cursor.remaining() {
                    return Err(MetagraphError::Malformed(
                        "compact column length exceeds input".into(),
                    ));
                }
                let mut values = Vec::with_capacity(count);
                for _ in 0..count {
                    let value = u64::try_from(compact_u128(&mut cursor)?).map_err(|_| {
                        MetagraphError::Malformed("column value exceeds u64".into())
                    })?;
                    if value > max {
                        return Err(MetagraphError::Malformed("score exceeds u16".into()));
                    }
                    values.push(value);
                }
                columns.compact.insert(name.into(), values);
                fields.push((name.into(), Value::Null));
            } else {
                let value = runtime.core().decode_id(field.ty.id, &mut cursor)?;
                if name == "identities" {
                    columns.other.insert(name.into(), value.clone());
                }
                fields.push((name.into(), value));
            }
        }
        if cursor.remaining() != 0 {
            return Err(MetagraphError::TrailingBytes);
        }
        Ok(Some(Self {
            raw,
            fields,
            columns,
        }))
    }

    /// Number of hotkeys, which determines Python's neuron row count.
    pub fn len(&self) -> usize {
        self.columns
            .fixed
            .get("hotkeys")
            .map_or(0, |(range, width)| range.len() / width)
    }

    /// Whether the response contains no neurons.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The borrowed hotkey bytes, exactly 32 bytes per neuron.
    pub fn hotkey_bytes(&self) -> &[u8] {
        self.columns
            .fixed
            .get("hotkeys")
            .and_then(|(range, _)| self.raw.get(range.clone()))
            .unwrap_or_default()
    }

    /// Materialize the decoder's full runtime-shaped record for comparison or export.
    pub fn to_dynamic(&self) -> Value {
        Value::record(
            self.fields
                .iter()
                .map(|(name, value)| {
                    (
                        name.clone(),
                        if self.columns.fixed.contains_key(name)
                            || self.columns.compact.contains_key(name)
                        {
                            self.columns.dynamic(self.raw, name)
                        } else {
                            value.clone()
                        },
                    )
                })
                .collect(),
        )
    }
}

fn axon(bytes: &[u8]) -> Result<Value> {
    let mut cursor = Cursor::new(bytes);
    let block = u64::from_le_bytes(
        cursor
            .take(8)?
            .try_into()
            .map_err(|_| MetagraphError::TrailingBytes)?,
    );
    let version = u32::from_le_bytes(
        cursor
            .take(4)?
            .try_into()
            .map_err(|_| MetagraphError::TrailingBytes)?,
    );
    let ip = u128::from_le_bytes(
        cursor
            .take(16)?
            .try_into()
            .map_err(|_| MetagraphError::TrailingBytes)?,
    );
    let port = u16::from_le_bytes(
        cursor
            .take(2)?
            .try_into()
            .map_err(|_| MetagraphError::TrailingBytes)?,
    );
    Ok(Value::record(vec![
        ("block".into(), Value::Int(i128::from(block))),
        ("version".into(), Value::Int(i128::from(version))),
        (
            "ip".into(),
            i128::try_from(ip).map_or(Value::Uint(ip), Value::Int),
        ),
        ("port".into(), Value::Int(i128::from(port))),
        ("ip_type".into(), Value::Int(i128::from(cursor.byte()?))),
        ("protocol".into(), Value::Int(i128::from(cursor.byte()?))),
        (
            "placeholder1".into(),
            Value::Int(i128::from(cursor.byte()?)),
        ),
        (
            "placeholder2".into(),
            Value::Int(i128::from(cursor.byte()?)),
        ),
    ]))
}
