//! Explicit compaction of finished operations (schema 4, D-120).
//!
//! A finished operation keeps its intent, so its ID stays idempotent, and its
//! exact finalized effect attempt, including signed bytes and material. Only
//! its other attempts lose their bytes and material; their IDs, hashes, roles
//! and terminal outcomes remain. Unfinished work, contradictory evidence and
//! anything unresolved are never compacted.
use super::{
    AttemptOutcome, AttemptRole, ExecutionMode, JOURNAL_READABLE_BY, JOURNAL_SCHEMA,
    JOURNAL_SCHEMA_MAX, JournalError, JournalImage, OperationIntent, OperationJournal,
    OperationRecord, SignedAttempt,
};
use rtao_primitives::H256;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, fmt};

/// Schema that introduced compacted operations.
const COMPACTED_SCHEMA: u32 = 4;

/// A finished operation reduced to its identity and finalized effect.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct CompactedOperation {
    /// Immutable application work; `begin` with the same intent returns
    /// [`JournalError::Compacted`], a different one [`JournalError::Identity`].
    pub intent: OperationIntent,
    /// Route the operation used.
    pub mode: ExecutionMode,
    /// The unique finalized effect, retained exactly as it was recorded.
    pub effect: SignedAttempt,
    /// Every other attempt, without its bytes or material.
    pub dropped: Vec<DroppedAttempt>,
    /// The operation record's own extensions, such as application markers.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub record_extensions: BTreeMap<String, Value>,
    /// Image revision that compacted the operation.
    pub revision: u64,
    /// Uninterpreted fields retained across supported writes.
    #[serde(default, flatten)]
    pub extensions: BTreeMap<String, Value>,
}
impl fmt::Debug for CompactedOperation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CompactedOperation")
            .field("intent", &self.intent)
            .field("effect", &self.effect)
            .field("dropped", &self.dropped.len())
            .finish_non_exhaustive()
    }
}

/// A non-effect attempt of a compacted operation: proven unexecuted, or a
/// carrier whose receipt never established the effect.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DroppedAttempt {
    /// Stable attempt ordinal.
    pub id: u64,
    /// Hash of the signed bytes that were dropped.
    pub hash: H256,
    /// The attempt's role.
    pub role: AttemptRole,
    /// Its terminal outcome; never `Unresolved`.
    pub outcome: AttemptOutcome,
    /// Uninterpreted fields retained across supported writes.
    #[serde(default, flatten)]
    pub extensions: BTreeMap<String, Value>,
}

/// Which finished operations [`OperationJournal::compact`] may compact.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct CompactionPolicy {
    /// Compact only effects finalized strictly below this height. Pass the
    /// finalized head minus the retention you want to keep whole.
    pub finalized_before: u64,
    /// Compact only this scope, or every scope.
    pub scope: Option<String>,
}
impl CompactionPolicy {
    /// Effects finalized strictly below `height`, in every scope.
    pub fn finalized_before(height: u64) -> Self {
        Self {
            finalized_before: height,
            scope: None,
        }
    }
    /// Restrict compaction to one scope.
    pub fn scope(mut self, scope: impl Into<String>) -> Self {
        self.scope = Some(scope.into());
        self
    }
}

/// What a compaction did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct CompactionReport {
    /// IDs compacted by this call.
    pub compacted: Vec<String>,
    /// Signed attempts whose bytes and material were dropped.
    pub dropped_attempts: usize,
    /// Operations left whole: unfinished, too recent, out of scope or not selected.
    pub retained: usize,
}

fn invalid() -> JournalError {
    JournalError::Storage("invalid compacted journal operation; original evidence retained".into())
}

pub(super) fn missing(image: &JournalImage, id: &str) -> JournalError {
    if image.compacted.contains_key(id) {
        JournalError::Compacted
    } else {
        JournalError::Identity
    }
}

/// The compacted form of a finished record, or `None` if it is not finished:
/// a unique uncontradicted finalized effect, every attempt terminal and no
/// retained semantic refusal.
pub(super) fn compacted_form(
    record: &OperationRecord,
    revision: u64,
) -> Result<Option<CompactedOperation>, JournalError> {
    let effect = match record.finalized_effect() {
        Ok(Some(effect)) => effect.clone(),
        Ok(None) | Err(JournalError::Blocked(_)) => return Ok(None),
        Err(error) => return Err(error),
    };
    if record.incompatibility.is_some()
        || record
            .attempts
            .iter()
            .any(|a| a.outcome == AttemptOutcome::Unresolved)
    {
        return Ok(None);
    }
    let dropped = record
        .attempts
        .iter()
        .filter(|a| a.id != effect.id)
        .map(|a| DroppedAttempt {
            id: a.id,
            hash: a.hash,
            role: a.role.clone(),
            outcome: a.outcome.clone(),
            extensions: BTreeMap::new(),
        })
        .collect();
    Ok(Some(CompactedOperation {
        intent: record.intent.clone(),
        mode: record.mode.clone(),
        effect,
        dropped,
        record_extensions: record.extensions.clone(),
        revision,
        extensions: BTreeMap::new(),
    }))
}

pub(super) fn validate(image: &JournalImage) -> Result<(), JournalError> {
    use blake2::{Blake2b, Digest, digest::consts::U32};
    if image.compacted.is_empty() {
        return Ok(());
    }
    if image.schema_version < COMPACTED_SCHEMA {
        return Err(invalid());
    }
    for (id, done) in &image.compacted {
        let effect = &done.effect;
        if id != &done.intent.id
            || image.operations.contains_key(id)
            || done.revision == 0
            || done.revision > image.revision
            || !matches!(effect.outcome, AttemptOutcome::Finalized { .. })
            || matches!(effect.role, AttemptRole::Carrier { .. })
            || !effect.evidence_conflicts.is_empty()
            || H256(Blake2b::<U32>::digest(&effect.bytes).into()) != effect.hash
        {
            return Err(invalid());
        }
        let mut ids = vec![effect.id];
        for dropped in &done.dropped {
            if dropped.outcome == AttemptOutcome::Unresolved
                || dropped.hash == effect.hash
                || (!matches!(dropped.role, AttemptRole::Carrier { .. })
                    && matches!(dropped.outcome, AttemptOutcome::Finalized { .. }))
            {
                return Err(invalid());
            }
            ids.push(dropped.id);
        }
        ids.sort_unstable();
        if ids.iter().enumerate().any(|(i, id)| *id != i as u64) {
            return Err(invalid());
        }
    }
    Ok(())
}

/// Compacted entries are immutable; an operation leaves `operations` only as
/// its exact compacted form; the schema never decreases.
pub(super) fn validate_transition(
    before: &JournalImage,
    after: &JournalImage,
) -> Result<(), JournalError> {
    if after.schema_version < before.schema_version
        || (after.schema_version > before.schema_version
            && after.schema_version > JOURNAL_SCHEMA_MAX)
        || after.readable_by()? < before.readable_by()?
    {
        return Err(invalid());
    }
    for (id, done) in &before.compacted {
        if after.compacted.get(id) != Some(done) {
            return Err(invalid());
        }
    }
    for (id, record) in &before.operations {
        if after.operations.contains_key(id) {
            continue;
        }
        let new = after.compacted.get(id).ok_or_else(invalid)?;
        if compacted_form(record, new.revision)?.as_ref() != Some(new)
            || new.revision != after.revision
        {
            return Err(invalid());
        }
    }
    Ok(())
}

impl OperationJournal {
    /// Compact finished operations selected by `policy` and `select` (D-120).
    ///
    /// An operation is finished when it has a unique, uncontradicted finalized
    /// effect (success or dispatch failure), every attempt is terminal and no
    /// semantic refusal is retained. Its intent and effect attempt stay whole in
    /// [`JournalImage::compacted`]; its other attempts keep only ID, hash, role
    /// and outcome. Unresolved work is never compacted. `select` lets an
    /// application keep records it still needs, for example weight work not yet
    /// handed to the reveal watcher.
    ///
    /// The first compaction raises the image to schema 4 (`readable_by` 4).
    /// Releases before D-120 refuse that image with an unsupported-schema error
    /// and leave it unchanged, so compact only once rolling back below this
    /// release is no longer needed. A call that selects nothing writes nothing.
    pub fn compact(
        &self,
        policy: &CompactionPolicy,
        select: impl Fn(&OperationRecord) -> bool,
    ) -> Result<CompactionReport, JournalError> {
        let chosen = |image: &JournalImage| -> Result<Vec<String>, JournalError> {
            let mut ids = Vec::new();
            for (id, record) in &image.operations {
                if policy
                    .scope
                    .as_ref()
                    .is_some_and(|scope| scope != &record.intent.scope)
                {
                    continue;
                }
                let Some(form) = compacted_form(record, 0)? else {
                    continue;
                };
                let AttemptOutcome::Finalized { number, .. } = form.effect.outcome else {
                    continue;
                };
                if number < policy.finalized_before && select(record) {
                    ids.push(id.clone());
                }
            }
            Ok(ids)
        };
        let snapshot = self.snapshot()?;
        if chosen(&snapshot)?.is_empty() {
            return Ok(CompactionReport {
                retained: snapshot.operations.len(),
                ..Default::default()
            });
        }
        self.update(|image| {
            let ids = chosen(image)?;
            let revision = image
                .revision
                .checked_add(1)
                .ok_or_else(|| JournalError::Storage("generation exhausted".into()))?;
            let mut report = CompactionReport::default();
            for id in ids {
                let record = image.operations.remove(&id).ok_or_else(invalid)?;
                let form = compacted_form(&record, revision)?.ok_or_else(invalid)?;
                report.dropped_attempts += form.dropped.len();
                image.compacted.insert(id.clone(), form);
                report.compacted.push(id);
            }
            report.retained = image.operations.len();
            if image.schema_version < COMPACTED_SCHEMA {
                debug_assert_eq!(image.schema_version, JOURNAL_SCHEMA);
                image.schema_version = COMPACTED_SCHEMA;
                image
                    .extensions
                    .insert(JOURNAL_READABLE_BY.into(), COMPACTED_SCHEMA.into());
            }
            Ok(report)
        })
    }
}
