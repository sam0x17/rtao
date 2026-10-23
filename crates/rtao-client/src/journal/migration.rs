//! Bounded raw-envelope migration before typed deserialization.
use super::{JOURNAL_SCHEMA, JournalError, JournalImage};
use serde_json::{Value, json};

// Validate object names while still reading bytes. Value's map deserializer
// otherwise collapses repeated decoded names before a preservation check sees
// them. A validation pass leaves actual numeric/Value decoding to serde_json.
struct UniqueNames;
impl<'de> serde::Deserialize<'de> for UniqueNames {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = UniqueNames;
            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("JSON with unique object names")
            }
            fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<UniqueNames, E> {
                Ok(UniqueNames)
            }
            fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<UniqueNames, E> {
                Ok(UniqueNames)
            }
            fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<UniqueNames, E> {
                Ok(UniqueNames)
            }
            fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<UniqueNames, E> {
                Ok(UniqueNames)
            }
            fn visit_str<E: serde::de::Error>(self, _: &str) -> Result<UniqueNames, E> {
                Ok(UniqueNames)
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<UniqueNames, E> {
                Ok(UniqueNames)
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<UniqueNames, A::Error> {
                while seq.next_element::<UniqueNames>()?.is_some() {}
                Ok(UniqueNames)
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<UniqueNames, A::Error> {
                let mut names = std::collections::BTreeSet::new();
                while let Some(name) = map.next_key::<String>()? {
                    if !names.insert(name) {
                        return Err(serde::de::Error::custom("duplicate object name"));
                    }
                    map.next_value::<UniqueNames>()?;
                }
                Ok(UniqueNames)
            }
        }
        deserializer.deserialize_any(Visitor)
    }
}

pub(super) fn parse_unique(bytes: &[u8]) -> Result<Value, JournalError> {
    // from_slice enforces EOF and serde_json's default recursion limit on BOTH
    // passes. The native opened-descriptor byte limit is checked before here.
    let error = || {
        JournalError::Storage(
            "invalid JSON or duplicate journal field; original bytes retained".into(),
        )
    };
    serde_json::from_slice::<UniqueNames>(bytes).map_err(|_| error())?;
    serde_json::from_slice(bytes).map_err(|_| error())
}

fn invalid() -> JournalError {
    JournalError::Storage("unsupported or lossy journal image; original bytes retained".into())
}
fn insert_new(value: &mut Value, name: &str, field: Value) -> Result<(), JournalError> {
    let object = value.as_object_mut().ok_or_else(invalid)?;
    // Legacy extensions may collide with newly reserved graph fields. Never
    // overwrite such evidence or infer that a legacy record was a shield pair.
    if object.contains_key(name) {
        return Err(invalid());
    }
    object.insert(name.into(), field);
    Ok(())
}
fn preserves(before: &Value, after: &Value) -> bool {
    match (before, after) {
        (Value::Object(old), Value::Object(new)) => old
            .iter()
            .all(|(k, v)| new.get(k).is_some_and(|w| preserves(v, w))),
        (Value::Array(old), Value::Array(new)) => {
            old.len() == new.len() && old.iter().zip(new).all(|(a, b)| preserves(a, b))
        }
        _ => before == after,
    }
}

/// Returns a validated schema3 image and whether it requires atomic publication.
/// Unknown nested fields must survive typed roundtripping; unsupported semantics
/// are refused before the original file can be replaced.
pub(super) fn decode(
    mut raw: Value,
    allow_migration: bool,
) -> Result<(JournalImage, bool), JournalError> {
    let version = raw
        .get("schema_version")
        .and_then(Value::as_u64)
        .and_then(|v| u32::try_from(v).ok())
        .ok_or_else(invalid)?;
    let migrated = version != JOURNAL_SCHEMA;
    if migrated {
        if !allow_migration || !matches!(version, 1 | 2) {
            return Err(JournalError::Schema(version));
        }
        let revision = raw
            .get("revision")
            .and_then(Value::as_u64)
            .and_then(|r| r.checked_add(1))
            .ok_or_else(invalid)?;
        let records = raw
            .get_mut("operations")
            .and_then(Value::as_object_mut)
            .ok_or_else(invalid)?;
        for record in records.values_mut() {
            let attempts = record
                .get_mut("attempts")
                .and_then(Value::as_array_mut)
                .ok_or_else(invalid)?;
            let mode = if attempts.is_empty() {
                "Unselected"
            } else {
                "Ordinary"
            };
            for (ordinal, attempt) in attempts.iter_mut().enumerate() {
                insert_new(
                    attempt,
                    "id",
                    json!(u64::try_from(ordinal).map_err(|_| invalid())?),
                )?;
                insert_new(attempt, "role", json!("Ordinary"))?;
                insert_new(attempt, "material_generation", Value::Null)?;
                insert_new(attempt, "evidence_conflicts", json!([]))?;
            }
            insert_new(record, "mode", json!(mode))?;
            insert_new(record, "shield_pairs", json!([]))?;
        }
        raw["schema_version"] = json!(JOURNAL_SCHEMA);
        raw["revision"] = json!(revision);
    }
    let image: JournalImage = serde_json::from_value(raw.clone()).map_err(|_| invalid())?;
    let roundtrip = serde_json::to_value(&image).map_err(|_| invalid())?;
    if !preserves(&raw, &roundtrip) {
        return Err(invalid());
    }
    image.validate()?;
    Ok((image, migrated))
}
