//! Persisted identities and conservative effect projection. No dispatch capability.
use super::{
    AttemptOutcome, JOURNAL_SCHEMA, JournalError, JournalImage, OperationRecord, SignedAttempt,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Logical work selects a route once, before its first signed attempt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExecutionMode {
    /// Scheduler work can exist before the executor chooses a route.
    Unselected,
    /// Ordinary signed-extrinsic submission.
    Ordinary,
    /// Paired signed-inner shield route. Dispatch remains unsupported in this slice.
    Shielded(ShieldPolicy),
}

/// Retained authorization limits, not runtime/protocol compatibility attestation.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShieldPolicy {
    /// Trusted adapter profile identifier; a string alone never qualifies a runtime.
    pub protocol: String,
    /// Originally selected mortal period (4 or 8 blocks).
    pub period: u64,
    /// Maximum distinct signed pairs, including abandoned preparation.
    pub maximum_pairs: u32,
    /// Uninterpreted policy evidence; never inferred as additional permissions.
    #[serde(default, flatten)]
    pub extensions: BTreeMap<String, Value>,
}
impl std::fmt::Debug for ShieldPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShieldPolicy")
            .field("period", &self.period)
            .field("maximum_pairs", &self.maximum_pairs)
            .finish_non_exhaustive()
    }
}

/// A physical attempt's role. Carrier receipts never establish the inner effect.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AttemptRole {
    /// The signed extrinsic itself is the logical effect.
    Ordinary,
    /// Exact signed inner of the referenced pair.
    Inner {
        /// Stable pair ordinal.
        pair: u64,
    },
    /// Only this encrypted carrier is eligible for shield author I/O.
    Carrier {
        /// Stable pair ordinal.
        pair: u64,
    },
}

/// Preparation phase is not an outcome or a transferable send permit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PairPhase {
    /// Inner published; no carrier or dispatch authorization.
    Preparing,
    /// Both signed components published; no dispatch authorization.
    Prepared,
    /// Durable uncertainty barrier. This representation cannot authorize I/O itself.
    DispatchAuthorized {
        /// Image revision that published authorization.
        revision: u64,
    },
    /// Never-authorized preparation explicitly retired by its fenced owner.
    Retired,
}

/// References into OperationRecord::attempts; no copied signed-attempt state.
#[derive(Clone, Serialize, Deserialize)]
pub struct ShieldPair {
    /// Stable zero-based pair ordinal. Never reused after cancellation/restart.
    pub id: u64,
    /// Exact material generation selected for both components.
    pub material_generation: u64,
    /// Stable inner attempt ID.
    pub inner: u64,
    /// Stable carrier attempt ID, absent during inner-only preparation.
    pub carrier: Option<u64>,
    /// Publication/authorization state, independent from receipts.
    pub phase: PairPhase,
    /// Uninterpreted binding evidence retained without logging it.
    #[serde(default, flatten)]
    pub extensions: BTreeMap<String, Value>,
}
impl std::fmt::Debug for ShieldPair {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShieldPair")
            .field("id", &self.id)
            .field("inner", &self.inner)
            .field("carrier", &self.carrier)
            .field("phase", &self.phase)
            .finish_non_exhaustive()
    }
}
fn invalid() -> JournalError {
    JournalError::Storage("invalid journal attempt graph; original evidence retained".into())
}

impl JournalImage {
    /// Validate a complete schema3 graph before adoption/publication.
    /// Unsupported or contradictory shapes never reset the image.
    pub fn validate(&self) -> Result<(), JournalError> {
        if self.schema_version != JOURNAL_SCHEMA {
            return Err(JournalError::Schema(self.schema_version));
        }
        for (id, record) in &self.operations {
            if id != &record.intent.id {
                return Err(invalid());
            }
            record.validate()?;
            for pair in &record.shield_pairs {
                if let PairPhase::DispatchAuthorized { revision } = pair.phase
                    && (revision == 0 || revision > self.revision)
                {
                    return Err(invalid());
                }
            }
        }
        Ok(())
    }
}
impl OperationRecord {
    pub(super) fn validate(&self) -> Result<(), JournalError> {
        use blake2::{Blake2b, Digest, digest::consts::U32};
        let mut hashes = std::collections::BTreeSet::new();
        for (ordinal, attempt) in self.attempts.iter().enumerate() {
            if attempt.id != ordinal as u64
                || !hashes.insert(attempt.hash.0)
                || rtao_primitives::H256(Blake2b::<U32>::digest(&attempt.bytes).into())
                    != attempt.hash
                || attempt
                    .material_generation
                    .is_some_and(|g| g > self.material_generation)
                || attempt.next < attempt.start
                || attempt.expiry.is_some_and(|expiry| expiry < attempt.start)
                || attempt
                    .evidence_conflicts
                    .contains(&AttemptOutcome::Unresolved)
            {
                return Err(invalid());
            }
        }
        match &self.mode {
            ExecutionMode::Unselected => {
                if !self.attempts.is_empty() || !self.shield_pairs.is_empty() {
                    return Err(invalid());
                }
            }
            ExecutionMode::Ordinary => {
                if !self.shield_pairs.is_empty()
                    || self
                        .attempts
                        .iter()
                        .any(|a| a.role != AttemptRole::Ordinary)
                {
                    return Err(invalid());
                }
            }
            ExecutionMode::Shielded(policy) => {
                if policy.protocol.is_empty()
                    || !matches!(policy.period, 4 | 8)
                    || policy.maximum_pairs == 0
                    || self.shield_pairs.len() as u64 > u64::from(policy.maximum_pairs)
                {
                    return Err(invalid());
                }
                let mut used = std::collections::BTreeSet::new();
                for (ordinal, pair) in self.shield_pairs.iter().enumerate() {
                    if pair.id != ordinal as u64 {
                        return Err(invalid());
                    }
                    let inner = self.attempt(pair.inner)?;
                    if inner.role != (AttemptRole::Inner { pair: pair.id })
                        || inner.material_generation != Some(pair.material_generation)
                        || !used.insert(inner.id)
                    {
                        return Err(invalid());
                    }
                    match (&pair.phase, pair.carrier) {
                        (PairPhase::Preparing, None) | (PairPhase::Retired, _) => {}
                        (PairPhase::Prepared | PairPhase::DispatchAuthorized { .. }, Some(_)) => {}
                        _ => return Err(invalid()),
                    }
                    if let Some(id) = pair.carrier {
                        let carrier = self.attempt(id)?;
                        if carrier.role != (AttemptRole::Carrier { pair: pair.id })
                            || carrier.material_generation != Some(pair.material_generation)
                            || !used.insert(carrier.id)
                            || inner.id >= carrier.id
                            || carrier.nonce.checked_add(1) != Some(inner.nonce)
                            || carrier.material != inner.material
                        {
                            return Err(invalid());
                        }
                    }
                }
                if used.len() != self.attempts.len() {
                    return Err(invalid());
                }
            }
        }
        Ok(())
    }
    /// Select an exact retained attempt without relying on vector tail position.
    pub fn attempt(&self, id: u64) -> Result<&SignedAttempt, JournalError> {
        let index = usize::try_from(id).map_err(|_| invalid())?;
        self.attempts
            .get(index)
            .filter(|a| a.id == id)
            .ok_or_else(invalid)
    }
    /// Finalized physical effect, if unique and uncontradicted. A carrier is excluded.
    /// Presence does not mean success: the historical receipt may report dispatch failure.
    pub fn finalized_effect(&self) -> Result<Option<&SignedAttempt>, JournalError> {
        self.validate()?;
        if self
            .attempts
            .iter()
            .any(|a| !a.evidence_conflicts.is_empty())
        {
            return Err(JournalError::Blocked(
                "conflicting finalized evidence".into(),
            ));
        }
        let mut effects = self.attempts.iter().filter(|a| {
            !matches!(a.role, AttemptRole::Carrier { .. })
                && matches!(a.outcome, AttemptOutcome::Finalized { .. })
        });
        let first = effects.next();
        if effects.next().is_some() {
            return Err(JournalError::Blocked(
                "multiple finalized effects retained".into(),
            ));
        }
        Ok(first)
    }
}

/// Merge evidence monotonically, by exact identity. Stale progress never erases
/// terminal receipts. Distinct terminal claims remain inspectable and block I/O.
pub(super) fn merge_outcome(attempt: &mut SignedAttempt, incoming: AttemptOutcome) {
    if incoming == AttemptOutcome::Unresolved || same_terminal(&incoming, &attempt.outcome) {
        return;
    }
    if attempt.outcome == AttemptOutcome::Unresolved {
        attempt.outcome = incoming;
    } else if !attempt
        .evidence_conflicts
        .iter()
        .any(|old| same_terminal(old, &incoming))
    {
        attempt.evidence_conflicts.push(incoming);
    }
}

fn same_terminal(a: &AttemptOutcome, b: &AttemptOutcome) -> bool {
    match (a, b) {
        (
            AttemptOutcome::Finalized {
                block: a,
                number: an,
                success: s,
                ..
            },
            AttemptOutcome::Finalized {
                block: b,
                number: bn,
                success: t,
                ..
            },
        ) => a == b && an == bn && s == t,
        _ => a == b,
    }
}

// Shape validation alone cannot make IDs stable: a writer could otherwise
// replace an old valid graph with a different valid graph and lose uncertainty.
pub(super) fn validate_transition(
    before: &JournalImage,
    after: &JournalImage,
) -> Result<(), JournalError> {
    after.validate()?;
    for (id, old) in &before.operations {
        let new = after.operations.get(id).ok_or_else(invalid)?;
        if old.intent != new.intent
            || old.attempts.len() > new.attempts.len()
            || old.shield_pairs.len() > new.shield_pairs.len()
            || (old.mode != new.mode && old.mode != ExecutionMode::Unselected)
            || new.material_generation < old.material_generation
            || (old.material_generation == new.material_generation && old.material != new.material)
        {
            return Err(invalid());
        }
        for added in &new.attempts[old.attempts.len()..] {
            if added.material_generation != Some(new.material_generation)
                || added.material != new.material
            {
                return Err(invalid());
            }
        }
        for (old, new) in old.attempts.iter().zip(&new.attempts) {
            let mut expected = old.clone();
            expected.next = new.next;
            expected.finalized = new.finalized;
            expected.outcome = new.outcome.clone();
            expected.evidence_conflicts = new.evidence_conflicts.clone();
            if expected != *new
                || new.next < old.next
                || (old.outcome != AttemptOutcome::Unresolved && old.outcome != new.outcome)
                || !new.evidence_conflicts.starts_with(&old.evidence_conflicts)
                || old.finalized.is_some_and(|(height, hash)| {
                    new.finalized
                        .is_none_or(|(next, id)| next < height || (next == height && hash != id))
                })
            {
                return Err(invalid());
            }
        }
        for (old, new) in old.shield_pairs.iter().zip(&new.shield_pairs) {
            let allowed = old.phase == new.phase
                || matches!(
                    (&old.phase, &new.phase),
                    (
                        PairPhase::Preparing,
                        PairPhase::Prepared | PairPhase::Retired
                    ) | (
                        PairPhase::Prepared,
                        PairPhase::DispatchAuthorized { .. } | PairPhase::Retired
                    )
                );
            if !allowed
                || old.id != new.id
                || old.inner != new.inner
                || old.material_generation != new.material_generation
                || if old.phase == PairPhase::Preparing && new.phase == PairPhase::Prepared {
                    old.extensions
                        .iter()
                        .any(|(key, value)| new.extensions.get(key) != Some(value))
                } else {
                    old.extensions != new.extensions
                }
                || (old.carrier != new.carrier
                    && !matches!(
                        (&old.phase, &new.phase, old.carrier, new.carrier),
                        (PairPhase::Preparing, PairPhase::Prepared, None, Some(_))
                    ))
            {
                return Err(invalid());
            }
        }
    }
    Ok(())
}
