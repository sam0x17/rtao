//! Shared journal account/operation preparation exclusion.
use super::*;

pub(super) struct SubmissionClaims {
    id: String,
    _account: JournalOwner,
    _operation: JournalOwner,
}

/// Exclusive account and logical-operation preparation lifetime.
///
/// This is not dispatch permission. Keep it alive while resolving keys and
/// submitting its scoped binding. Native author custody may retain its private
/// strong lifetime references after caller cancellation. A stale scoped binding
/// cannot silently reacquire expired ownership.
pub struct JournalPreparation {
    binding: JournalBinding,
    pub(super) claims: Arc<SubmissionClaims>,
}
impl JournalPreparation {
    /// Binding scoped to this owner's lifetime, retaining the selected material
    /// and semantic/protocol guards. The binding holds only a weak reference.
    pub fn binding(&self) -> JournalBinding {
        let mut binding = self.binding.clone();
        binding.preparation_owner = Some(Arc::downgrade(&self.claims));
        binding
    }
    pub(crate) fn author_ownership(&self, hash: H256) -> rtao_rpc::AuthorOwnership {
        rtao_rpc::AuthorOwnership::new(rtao_rpc::H256(hash.0), self.claims.clone()).0
    }
}
impl JournalBinding {
    /// Reserve account then operation, across clients/IDs sharing this journal.
    /// Scheduler ownership remains a distinct outer scope. This performs no
    /// signing or author I/O and does not replace mode/pause/readiness checks.
    pub fn reserve(&self) -> Result<JournalPreparation, JournalError> {
        let claims = self.claim_submission()?;
        Ok(JournalPreparation {
            binding: self.clone(),
            claims,
        })
    }
    pub(super) fn claim_submission(&self) -> Result<Arc<SubmissionClaims>, JournalError> {
        if let Some(owner) = &self.preparation_owner {
            let claims = owner.upgrade().ok_or(JournalError::Owned)?;
            if claims.id != self.id {
                return Err(JournalError::Identity);
            }
            return Ok(claims);
        }
        let record = self.record()?;
        let account_scope = format!(
            "journal-account/{}/{}",
            crate::hash_hex(record.intent.genesis),
            hex::encode(record.intent.account.0)
        );
        let account = self.journal.claim(account_scope)?;
        let operation = self
            .journal
            .claim(format!("journal-operation/{}", self.id))?;
        let image = self.journal.snapshot()?;
        let current = image
            .operations
            .get(&self.id)
            .ok_or(JournalError::Identity)?;
        if current.material_generation != self.material_generation {
            return Err(JournalError::Conflict);
        }
        for other in image.operations.values() {
            if other.intent.id != self.id
                && other.intent.genesis == record.intent.genesis
                && other.intent.account == record.intent.account
                && pending_author(other)
            {
                return Err(JournalError::Blocked(
                    "another account operation needs reconciliation".into(),
                ));
            }
        }
        Ok(Arc::new(SubmissionClaims {
            id: self.id.clone(),
            _account: account,
            _operation: operation,
        }))
    }
}
fn pending_author(record: &OperationRecord) -> bool {
    if record
        .attempts
        .iter()
        .any(|a| !a.evidence_conflicts.is_empty())
    {
        return true;
    }
    match record.finalized_effect() {
        Ok(Some(_)) => return false,
        Err(_) => return true,
        Ok(None) => {}
    }
    match record.mode {
        ExecutionMode::Unselected | ExecutionMode::Ordinary => record
            .attempts
            .iter()
            .any(|a| a.outcome == AttemptOutcome::Unresolved),
        ExecutionMode::Shielded(_) => record.shield_pairs.iter().any(|pair| {
            pair.phase != PairPhase::Retired
                && std::iter::once(pair.inner).chain(pair.carrier).any(|id| {
                    record
                        .attempt(id)
                        .is_ok_and(|a| a.outcome == AttemptOutcome::Unresolved)
                })
        }),
    }
}
