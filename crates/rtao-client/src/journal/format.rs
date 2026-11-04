//! Format versions and the declared compatibility range (D-120).
//!
//! Every persisted shape names the format it was written in and may declare
//! the oldest format whose readers can still read it (`readable_by`). A
//! reader accepts its own and older formats strictly, and a newer one only
//! when the writer declared it compatible; anything else is refused with a
//! version error and left unchanged. A writer raises `readable_by` whenever an
//! older reader that ignored its new fields could act wrongly.
use super::{JOURNAL_READABLE_BY, JOURNAL_SCHEMA, JOURNAL_SCHEMA_MAX, JournalError, JournalImage};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;

fn invalid(what: &str) -> JournalError {
    JournalError::Storage(format!(
        "malformed {what} format declaration; original bytes retained"
    ))
}

fn declared(value: Option<&Value>, what: &str) -> Result<Option<u32>, JournalError> {
    value
        .map(|v| {
            v.as_u64()
                .and_then(|v| u32::try_from(v).ok())
                .filter(|v| *v > 0)
                .ok_or_else(|| invalid(what))
        })
        .transpose()
}

/// Gate an envelope by its schema and declared floor before and after typed
/// decoding. Schemas 1 and 2 are refused here; only the native opener
/// migrates them, before this check.
pub(super) fn check_schema(schema: u32, readable_by: Option<&Value>) -> Result<(), JournalError> {
    let floor = declared(readable_by, "journal")?.unwrap_or(schema);
    if floor > schema {
        return Err(invalid("journal"));
    }
    match schema {
        JOURNAL_SCHEMA..=JOURNAL_SCHEMA_MAX => Ok(()),
        newer if newer > JOURNAL_SCHEMA_MAX => {
            if floor <= JOURNAL_SCHEMA_MAX {
                Ok(())
            } else {
                Err(JournalError::Newer {
                    record: "journal".into(),
                    version: newer,
                    readable_by: floor,
                    supported: JOURNAL_SCHEMA_MAX,
                })
            }
        }
        older => Err(JournalError::Schema(older)),
    }
}

impl JournalImage {
    /// Oldest schema whose readers may read and update this image (D-120):
    /// the declared [`JOURNAL_READABLE_BY`] value, or `schema_version`.
    pub fn readable_by(&self) -> Result<u32, JournalError> {
        Ok(
            declared(self.extensions.get(JOURNAL_READABLE_BY), "journal")?
                .unwrap_or(self.schema_version),
        )
    }
    /// Check the schema against this build's compatibility range (D-120).
    pub fn check_schema(&self) -> Result<(), JournalError> {
        check_schema(
            self.schema_version,
            self.extensions.get(JOURNAL_READABLE_BY),
        )
    }
}

/// Format rule of an SDK-owned record kept inside a journal, such as weight
/// work, attempt material or a scheduler checkpoint (D-120).
///
/// The record is a JSON object whose `key` names its format (absent means 1)
/// and whose optional `readable_by` names the oldest compatible format
/// (absent means the format itself). [`Self::decode`] reads a record at or
/// below `current` strictly: every field must be known, so an unknown field
/// is refused rather than ignored. A newer record is read only when it
/// declares `readable_by <= current`; its unknown fields are then ignored,
/// as its writer declared safe. Otherwise it returns [`JournalError::Newer`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecordFormat {
    /// Record kind used in errors, such as `weight work`.
    pub name: &'static str,
    /// Object key holding the format version.
    pub key: &'static str,
    /// Newest format this build reads and the one it writes.
    pub current: u32,
}

impl RecordFormat {
    /// Key of the declared compatibility floor inside a record.
    pub const READABLE_BY: &'static str = "readable_by";

    /// A format rule; `current` must be at least 1.
    pub const fn new(name: &'static str, key: &'static str, current: u32) -> Self {
        Self { name, key, current }
    }

    /// The record's `(format, readable_by)` declaration.
    pub fn version(&self, value: &Value) -> Result<(u32, u32), JournalError> {
        let Some(object) = value.as_object() else {
            return Ok((1, 1));
        };
        let version = declared(object.get(self.key), self.name)?.unwrap_or(1);
        let floor = declared(object.get(Self::READABLE_BY), self.name)?.unwrap_or(version);
        if floor > version {
            return Err(invalid(self.name));
        }
        Ok((version, floor))
    }

    /// Decode a record under this rule. See the type documentation.
    pub fn decode<T: DeserializeOwned + Serialize>(
        &self,
        value: &Value,
    ) -> Result<T, JournalError> {
        let (version, floor) = self.version(value)?;
        if version > self.current && floor > self.current {
            return Err(JournalError::Newer {
                record: self.name.into(),
                version,
                readable_by: floor,
                supported: self.current,
            });
        }
        let mut body = value.clone();
        if let Some(object) = body.as_object_mut() {
            object.remove(self.key);
            object.remove(Self::READABLE_BY);
        }
        let unreadable = || {
            JournalError::Storage(format!(
                "{} record does not match format {version}; original retained",
                self.name
            ))
        };
        let decoded: T = serde_json::from_value(body.clone()).map_err(|_| unreadable())?;
        if version <= self.current {
            // Strict: a field this format does not define is refused, never dropped.
            let roundtrip = serde_json::to_value(&decoded).map_err(|_| unreadable())?;
            if !super::migration::preserves(&body, &roundtrip) {
                return Err(unreadable());
            }
        }
        Ok(decoded)
    }
}
