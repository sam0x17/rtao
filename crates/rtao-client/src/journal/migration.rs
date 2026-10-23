//! Bounded raw-envelope migration before typed deserialization.
use super::{JOURNAL_SCHEMA, JournalError, JournalImage};
use serde_json::{Value, json};

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
