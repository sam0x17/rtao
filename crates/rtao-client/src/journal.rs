//! Durable logical operations and separately recorded signed attempts.
//!
//! Storage methods are synchronous; native callers should run them on an I/O
//! worker. Signed-attempt and receipt publication are offloaded automatically;
//! synchronous readiness checks and scheduler checkpoints still perform I/O.
//! No signing key is stored.
//! Signed bytes and commit material are sensitive and are redacted from Debug.

mod graph;
mod migration;
mod native;
pub use graph::{AttemptRole, ExecutionMode, PairPhase, ShieldPair, ShieldPolicy};
#[cfg(test)]
mod tests;
pub use native::NativeJournalStore;

use crate::{Client, ExtrinsicReport, UnsignedExtrinsic};
use rtao_primitives::{AccountId, H256};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    sync::{Arc, Mutex},
};

/// Current storage envelope version. Older supported versions migrate atomically.
pub const JOURNAL_SCHEMA: u32 = 3;
/// A storage, identity, concurrency or recovery refusal. Payloads are never included.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JournalError {
    /// Storage failed; no dispatch is authorized by a failed write.
    Storage(String),
    /// Another writer changed the expected generation.
    Conflict,
    /// A scheduler or process already owns this journal scope.
    Owned,
    /// The operation ID was reused with different intent or chain identity.
    Identity,
    /// New dispatch is paused or an earlier attempt is unresolved/finished.
    Blocked(String),
    /// The envelope is not a supported storage generation.
    Schema(u32),
}
impl fmt::Display for JournalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Storage(detail) => write!(f, "journal storage failure: {detail}"),
            Self::Conflict => f.write_str("journal generation conflict"),
            Self::Owned => f.write_str("journal scope is already owned"),
            Self::Identity => f.write_str("journal operation identity differs"),
            Self::Blocked(reason) => write!(f, "journal dispatch blocked: {reason}"),
            Self::Schema(version) => write!(f, "unsupported journal schema {version}"),
        }
    }
}
impl std::error::Error for JournalError {}

/// Application-supplied semantic incompatibility; independent of spec equality.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Incompatibility {
    /// Stable adapter-specific reason token.
    pub code: String,
    /// Optional runtime descriptor involved.
    pub descriptor: Option<String>,
    /// Operator-readable remedy, without secret material.
    pub detail: String,
    /// Uninterpreted fields retained across supported storage migrations.
    #[serde(default, flatten)]
    pub extensions: BTreeMap<String, Value>,
}

/// Optional semantic authorization immediately before signing in the current runtime.
pub trait SemanticGuard: Send + Sync {
    /// Check application meaning, with access to fresh state and signing context.
    fn check<'a>(
        &'a self,
        client: &'a Client,
        intent: &'a OperationIntent,
        unsigned: &'a UnsignedExtrinsic,
    ) -> rtao_signer::BoxFuture<'a, Result<(), Incompatibility>>;
}

/// Immutable logical work, independent of any signed transaction's lifetime.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct OperationIntent {
    /// Stable application-selected logical ID; never reused for another effect.
    pub id: String,
    /// Optional caller correlation ID; the SDK assigns it no business semantics.
    pub correlation_id: Option<String>,
    /// Genesis that authorizes this work.
    pub genesis: H256,
    /// Outer signing account.
    pub account: AccountId,
    /// SDK or application namespace, such as one weight mechanism.
    pub scope: String,
    /// Logical effect and input, retained across rebuilding physical attempts.
    pub effects: Value,
    /// True only for a previously authorized obligation permitted during pause.
    pub obligation: bool,
    /// Optional stable semantic contract. Recovery must reinstall its guard.
    #[serde(default)]
    pub semantic_policy: Option<String>,
    /// Uninterpreted fields retained without logging or inferring authorization.
    #[serde(default, flatten)]
    pub extensions: BTreeMap<String, Value>,
}
impl fmt::Debug for OperationIntent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OperationIntent")
            .field("id", &self.id)
            .field("scope", &self.scope)
            .finish_non_exhaustive()
    }
}

/// A finalized historical disposition, not an RPC or timer inference.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AttemptOutcome {
    /// Persisted before dispatch. A crash may have happened on either side of I/O.
    Unresolved,
    /// Complete finalized history excluded these exact bytes through their era.
    ExpiredUnexecuted,
    /// Finalized inclusion, including a dispatch failure with possible partial effects.
    Finalized {
        /// Historical inclusion block.
        block: H256,
        /// Height of the historical inclusion.
        number: u64,
        /// Runtime dispatch status. Failure does not authorize blind replacement.
        success: bool,
        /// Uninterpreted historical evidence retained across migration.
        #[serde(default, flatten)]
        extensions: BTreeMap<String, Value>,
    },
}
impl fmt::Debug for AttemptOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unresolved => f.write_str("Unresolved"),
            Self::ExpiredUnexecuted => f.write_str("ExpiredUnexecuted"),
            Self::Finalized {
                block,
                number,
                success,
                ..
            } => f
                .debug_struct("Finalized")
                .field("block", block)
                .field("number", number)
                .field("success", success)
                .finish_non_exhaustive(),
        }
    }
}
/// An exact signed attempt. Debug deliberately omits its bytes and material.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedAttempt {
    /// Stable zero-based ordinal. Attempts are never removed or reordered.
    pub id: u64,
    /// Physical identity's role in the logical effect.
    pub role: AttemptRole,
    /// Exact material selection; unknown for migrated ordinary attempts.
    pub material_generation: Option<u64>,
    /// Hash of exactly the persisted signed bytes.
    pub hash: H256,
    /// SCALE signed extrinsic. Never print this field in logs.
    pub bytes: Vec<u8>,
    /// Allocated account nonce.
    pub nonce: u64,
    /// Runtime spec used to sign.
    pub spec_version: u32,
    /// Transaction encoding version used to sign.
    pub transaction_version: u32,
    /// Finalized-history lower bound captured before dispatch.
    pub start: u64,
    /// First height after mortal validity; None is immortal and cannot expire.
    pub expiry: Option<u64>,
    /// Era anchor used by the signature.
    pub era_anchor: H256,
    /// Exact normalized mortality era used in the signed encoding.
    /// Null denotes a legacy view; signed bytes/start/expiry remain retained.
    #[serde(default)]
    pub era: Value,
    /// Runtime metadata hash, when enabled in the signature.
    pub metadata_hash: Option<[u8; 32]>,
    /// Snapshot of required operation-specific commit/reveal material.
    pub material: Value,
    /// Next unscanned finalized height. Persisted only after a successful body read.
    pub next: u64,
    /// Last reconciled finalized identity, used to refuse stale/forked RPC views.
    pub finalized: Option<(u64, H256)>,
    /// Historical result, initially unresolved.
    pub outcome: AttemptOutcome,
    /// Contradictory terminal evidence, retained instead of silently overwritten.
    pub evidence_conflicts: Vec<AttemptOutcome>,
    /// Uninterpreted attempt fields survive supported migration and later writes.
    #[serde(default, flatten)]
    pub extensions: BTreeMap<String, Value>,
}
impl fmt::Debug for SignedAttempt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SignedAttempt")
            .field("hash", &self.hash)
            .field("nonce", &self.nonce)
            .field("outcome", &self.outcome)
            .finish_non_exhaustive()
    }
}
/// Persisted operation including every attempt; unresolved records are never pruned.
#[derive(Clone, Serialize, Deserialize)]
pub struct OperationRecord {
    /// Route selection is durable before signing; it cannot change afterwards.
    pub mode: ExecutionMode,
    /// References into the existing attempts vector, not another attempt tracker.
    pub shield_pairs: Vec<ShieldPair>,
    /// Immutable application work.
    pub intent: OperationIntent,
    /// Preparation/reveal material, persisted before signing or network dispatch.
    pub material: Value,
    /// Monotonic material selection; existing images start at generation zero.
    #[serde(default)]
    pub material_generation: u64,
    /// Ordered physical attempts, retained after completion.
    pub attempts: Vec<SignedAttempt>,
    /// Last application semantic refusal, permitting update and resumption.
    pub incompatibility: Option<Incompatibility>,
    /// Unknown record extensions survive supported migrations.
    #[serde(default, flatten)]
    pub extensions: BTreeMap<String, Value>,
}
impl fmt::Debug for OperationRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OperationRecord")
            .field("intent", &self.intent)
            .field("attempts", &self.attempts.len())
            .finish_non_exhaustive()
    }
}
/// Explicit pause policy. Observation and reconciliation are always allowed.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalControl {
    /// Stop dispatch and accepting new logical operations.
    pub submissions_paused: bool,
    /// Permit existing explicitly authorized obligations while new work is paused.
    pub allow_obligations: bool,
    /// Uninterpreted control fields retained, never treated as permissions.
    #[serde(default, flatten)]
    pub extensions: BTreeMap<String, Value>,
}
impl fmt::Debug for JournalControl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JournalControl")
            .field("submissions_paused", &self.submissions_paused)
            .field("allow_obligations", &self.allow_obligations)
            .finish_non_exhaustive()
    }
}
/// Versioned image passed through a storage adapter's atomic CAS boundary.
#[derive(Clone, Serialize, Deserialize)]
pub struct JournalImage {
    /// Storage envelope version.
    pub schema_version: u32,
    /// Monotonic CAS generation, unrelated to chain height.
    pub revision: u64,
    /// Logical ID index; completed IDs also remain idempotent.
    pub operations: BTreeMap<String, OperationRecord>,
    /// SDK-owned snapshots, separate from caller effect data.
    #[serde(default)]
    pub checkpoints: BTreeMap<String, Value>,
    /// New-submission control independent from observation.
    #[serde(default)]
    pub control: JournalControl,
    /// Preserve unknown envelope fields during supported migrations.
    #[serde(default, flatten)]
    pub extensions: BTreeMap<String, Value>,
}
impl Default for JournalImage {
    fn default() -> Self {
        Self {
            schema_version: JOURNAL_SCHEMA,
            revision: 0,
            operations: BTreeMap::new(),
            checkpoints: BTreeMap::new(),
            control: JournalControl::default(),
            extensions: BTreeMap::new(),
        }
    }
}
/// Durable storage contract. A successful CAS must be durable before returning.
///
/// Implementations must serialize processes and compare the complete generation;
/// unknown schemas and ambiguous storage errors must fail closed, never reset.
pub trait JournalStore: Send + Sync {
    /// Acquire an exclusive scope across every journal handle using this store.
    /// The guard must remain valid until drop; a timed lease alone is insufficient
    /// unless every write and dispatch is additionally fenced by the adapter.
    fn claim_scope(&self, scope: &str) -> Result<Box<dyn JournalLease>, JournalError>;
    /// Read a complete consistent image.
    fn load(&self) -> Result<JournalImage, JournalError>;
    /// Atomically replace exactly the expected generation with expected+1.
    fn compare_exchange(&self, expected: u64, image: &JournalImage) -> Result<(), JournalError>;
}
/// Opaque ownership guard returned by a storage adapter.
pub trait JournalLease: Send + Sync {}
impl<T: Send + Sync> JournalLease for T {}

/// Shared journal over an injected versioned storage adapter.
pub struct OperationJournal {
    store: Arc<dyn JournalStore>,
    owners: Mutex<BTreeSet<String>>,
}
impl OperationJournal {
    /// Adopt a versioned adapter; unsupported schemas are refused.
    pub fn new(store: Arc<dyn JournalStore>) -> Result<Arc<Self>, JournalError> {
        let image = store.load()?;
        image.validate()?;
        Ok(Arc::new(Self {
            store,
            owners: Mutex::new(BTreeSet::new()),
        }))
    }
    /// Open the native exclusive, atomic-file backend.
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Arc<Self>, JournalError> {
        Self::new(Arc::new(NativeJournalStore::open(path)?))
    }
    /// Inspect all operations, including unresolved and finalized work.
    pub fn snapshot(&self) -> Result<JournalImage, JournalError> {
        let image = self.store.load()?;
        image.validate()?;
        Ok(image)
    }
    /// Atomically update a complete image. A conflict never authorizes dispatch.
    pub fn update<T>(
        &self,
        f: impl FnOnce(&mut JournalImage) -> Result<T, JournalError>,
    ) -> Result<T, JournalError> {
        let mut image = self.snapshot()?;
        let before = image.revision;
        let prior = image.clone();
        let result = f(&mut image)?;
        image.revision = before
            .checked_add(1)
            .ok_or_else(|| JournalError::Storage("generation exhausted".into()))?;
        graph::validate_transition(&prior, &image)?;
        self.store.compare_exchange(before, &image)?;
        Ok(result)
    }
    /// Exclusively claim a scheduler scope, including against clones of this handle.
    pub fn claim(self: &Arc<Self>, scope: impl Into<String>) -> Result<JournalOwner, JournalError> {
        let scope = scope.into();
        if !self
            .owners
            .lock()
            .map_err(|_| JournalError::Owned)?
            .insert(scope.clone())
        {
            return Err(JournalError::Owned);
        }
        let lease = match self.store.claim_scope(&scope) {
            Ok(lease) => lease,
            Err(error) => {
                if let Ok(mut owners) = self.owners.lock() {
                    owners.remove(&scope);
                }
                return Err(error);
            }
        };
        Ok(JournalOwner {
            journal: Arc::clone(self),
            scope,
            _lease: lease,
        })
    }
    /// Set pause control without stopping finalized observation.
    pub fn control(&self, mut control: JournalControl) -> Result<(), JournalError> {
        self.update(|image| {
            for (key, value) in &image.control.extensions {
                control
                    .extensions
                    .entry(key.clone())
                    .or_insert_with(|| value.clone());
            }
            image.control = control;
            Ok(())
        })
    }
    /// Create or reopen exactly the same logical work; changed effects are refused.
    pub fn begin(
        self: &Arc<Self>,
        intent: OperationIntent,
    ) -> Result<JournalBinding, JournalError> {
        if intent.id.is_empty() || intent.id.len() > 256 || intent.scope.is_empty() {
            return Err(JournalError::Identity);
        }
        let material_generation = self.update(|image| {
            if let Some(record) = image.operations.get(&intent.id) {
                if record.intent != intent {
                    return Err(JournalError::Identity);
                }
            } else {
                if image.control.submissions_paused {
                    return Err(JournalError::Blocked("new operations paused".into()));
                }
                image.operations.insert(
                    intent.id.clone(),
                    OperationRecord {
                        mode: ExecutionMode::Unselected,
                        shield_pairs: Vec::new(),
                        intent: intent.clone(),
                        material: Value::Null,
                        material_generation: 0,
                        attempts: Vec::new(),
                        incompatibility: None,
                        extensions: BTreeMap::new(),
                    },
                );
            }
            Ok(image
                .operations
                .get(&intent.id)
                .ok_or(JournalError::Identity)?
                .material_generation)
        })?;
        Ok(JournalBinding {
            journal: Arc::clone(self),
            id: intent.id,
            guard: None,
            material_generation,
        })
    }
    /// Bind a persisted ID without changing its effects or authorization.
    pub fn bind(self: &Arc<Self>, id: &str) -> Result<JournalBinding, JournalError> {
        let material_generation = self
            .snapshot()?
            .operations
            .get(id)
            .ok_or(JournalError::Identity)?
            .material_generation;
        Ok(JournalBinding {
            journal: Arc::clone(self),
            id: id.to_owned(),
            guard: None,
            material_generation,
        })
    }
}
/// Scope ownership lasts until this guard is dropped; it has no expiry refund.
pub struct JournalOwner {
    journal: Arc<OperationJournal>,
    scope: String,
    _lease: Box<dyn JournalLease>,
}
impl Drop for JournalOwner {
    fn drop(&mut self) {
        if let Ok(mut owners) = self.journal.owners.lock() {
            owners.remove(&self.scope);
        }
    }
}

/// One logical operation's authorization carried into the existing submission path.
#[derive(Clone)]
pub struct JournalBinding {
    journal: Arc<OperationJournal>,
    id: String,
    guard: Option<Arc<dyn SemanticGuard>>,
    material_generation: u64,
}
impl fmt::Debug for JournalBinding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JournalBinding")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}
impl JournalBinding {
    /// Set an application-specific semantic gate; spec changes alone do not reject.
    pub fn with_guard(mut self, guard: Arc<dyn SemanticGuard>) -> Self {
        self.guard = Some(guard);
        self
    }
    /// Logical operation identity.
    pub fn id(&self) -> &str {
        &self.id
    }
    /// Read its retained evidence without mutating the operation.
    pub fn record(&self) -> Result<OperationRecord, JournalError> {
        self.journal
            .snapshot()?
            .operations
            .remove(&self.id)
            .ok_or(JournalError::Identity)
    }
    /// Persist and select required material before preparing/signing its attempt.
    /// Use the returned binding. Old clones fail closed after any replacement,
    /// including an ABA replacement with identical material bytes.
    pub fn material(mut self, material: Value) -> Result<Self, JournalError> {
        self.material_generation = self.journal.update(|image| {
            let record = image
                .operations
                .get_mut(&self.id)
                .ok_or(JournalError::Identity)?;
            dispatchable(record, &image.control)?;
            if record.material_generation != self.material_generation {
                return Err(JournalError::Conflict);
            }
            record.material_generation = record
                .material_generation
                .checked_add(1)
                .ok_or_else(|| JournalError::Storage("material generation exhausted".into()))?;
            record.material = material;
            Ok(record.material_generation)
        })?;
        Ok(self)
    }
    /// Check pause and previous attempt state before obtaining a private signer.
    pub fn can_dispatch(&self) -> Result<(), JournalError> {
        let image = self.journal.snapshot()?;
        let record = image
            .operations
            .get(&self.id)
            .ok_or(JournalError::Identity)?;
        dispatchable(record, &image.control)?;
        if record.material_generation != self.material_generation {
            return Err(JournalError::Conflict);
        }
        Ok(())
    }
    pub(crate) async fn check(
        &self,
        client: &Client,
        unsigned: &UnsignedExtrinsic,
    ) -> Result<(), JournalError> {
        self.can_dispatch()?;
        let record = self.record()?;
        if record.material_generation != self.material_generation {
            return Err(JournalError::Conflict);
        }
        if record.intent.genesis != unsigned.genesis_hash()
            || record.intent.account != unsigned.address()
        {
            return Err(JournalError::Identity);
        }
        if record.intent.semantic_policy.is_some() && self.guard.is_none() {
            return Err(JournalError::Blocked(
                "reinstall the operation's semantic guard before signing".into(),
            ));
        }
        if let Some(guard) = &self.guard
            && let Err(reason) = guard.check(client, &record.intent, unsigned).await
        {
            self.journal.update(|image| {
                image
                    .operations
                    .get_mut(&self.id)
                    .ok_or(JournalError::Identity)?
                    .incompatibility = Some(reason.clone());
                Ok(())
            })?;
            return Err(JournalError::Blocked(format!(
                "semantic incompatibility: {}",
                reason.code
            )));
        }
        Ok(())
    }
    pub(crate) async fn signed(
        &self,
        unsigned: &UnsignedExtrinsic,
        bytes: &[u8],
        hash: H256,
    ) -> Result<(), JournalError> {
        let binding = self.clone();
        let attempt = SignedAttempt {
            id: 0, // Assigned under the durable CAS, never by a signing task.
            role: AttemptRole::Ordinary,
            material_generation: Some(self.material_generation),
            hash,
            bytes: bytes.to_vec(),
            nonce: unsigned.nonce(),
            spec_version: unsigned.call().stamp().spec_version,
            transaction_version: unsigned.call().stamp().transaction_version,
            start: unsigned.submission_head(),
            expiry: unsigned.expiry(),
            era_anchor: unsigned.era_block_hash(),
            era: rtao_core::codec::value::to_corpus_json(unsigned.era()),
            metadata_hash: unsigned.metadata_hash(),
            material: Value::Null,
            next: unsigned.submission_head(),
            finalized: None,
            outcome: AttemptOutcome::Unresolved,
            evidence_conflicts: Vec::new(),
            extensions: BTreeMap::new(),
        };
        crate::offload(move || {
            binding.journal.update(|image| {
                let record = image
                    .operations
                    .get_mut(&binding.id)
                    .ok_or(JournalError::Identity)?;
                dispatchable(record, &image.control)?;
                if record.material_generation != binding.material_generation {
                    return Err(JournalError::Conflict);
                }
                if !matches!(
                    record.mode,
                    ExecutionMode::Unselected | ExecutionMode::Ordinary
                ) {
                    return Err(JournalError::Blocked(
                        "shield dispatch adapter unavailable".into(),
                    ));
                }
                record.mode = ExecutionMode::Ordinary;
                let mut attempt = attempt;
                attempt.id = u64::try_from(record.attempts.len())
                    .map_err(|_| JournalError::Storage("attempt ordinal exhausted".into()))?;
                attempt.material = record.material.clone();
                record.attempts.push(attempt);
                record.incompatibility = None;
                Ok(())
            })
        })
        .await
        .map_err(|_| JournalError::Storage("journal worker stopped".into()))?
    }
    pub(crate) async fn observed(&self, report: &ExtrinsicReport) -> Result<(), JournalError> {
        if report.inclusion != crate::Inclusion::Finalized {
            return Ok(());
        }
        let block = report
            .block_hash
            .ok_or_else(|| JournalError::Blocked("missing finalized inclusion".into()))?;
        let number = report
            .block_number
            .ok_or_else(|| JournalError::Blocked("missing finalized height".into()))?;
        let binding = self.clone();
        let hash = report.hash;
        let success = report.success;
        crate::offload(move || {
            binding.journal.update(|image| {
                let attempt = image
                    .operations
                    .get_mut(&binding.id)
                    .and_then(|r| r.attempts.iter_mut().find(|a| a.hash == hash))
                    .ok_or(JournalError::Identity)?;
                graph::merge_outcome(
                    attempt,
                    AttemptOutcome::Finalized {
                        block,
                        number,
                        success,
                        extensions: BTreeMap::new(),
                    },
                );
                Ok(())
            })
        })
        .await
        .map_err(|_| JournalError::Storage("journal worker stopped".into()))?
    }
}
fn dispatchable(record: &OperationRecord, control: &JournalControl) -> Result<(), JournalError> {
    if matches!(record.mode, ExecutionMode::Shielded(_)) {
        return Err(JournalError::Blocked(
            "shield dispatch adapter unavailable".into(),
        ));
    }
    if control.submissions_paused && !(control.allow_obligations && record.intent.obligation) {
        return Err(JournalError::Blocked(
            "submissions paused; observation remains enabled".into(),
        ));
    }
    if record
        .attempts
        .iter()
        .any(|a| a.outcome != AttemptOutcome::ExpiredUnexecuted || !a.evidence_conflicts.is_empty())
    {
        return Err(JournalError::Blocked(
            "reconcile existing attempts before replacement".into(),
        ));
    }
    Ok(())
}

/// Why finalized recovery cannot yet authorize another attempt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecoveryReason {
    /// The current finalized endpoint cannot be read.
    FinalizedUnavailable,
    /// Contradictory terminal evidence is retained; author I/O remains blocked.
    EvidenceConflict,
    /// Paired dispatch and historical protocol qualification are not enabled.
    ShieldAdapterRequired,
    /// A concurrent writer published this still-unresolved physical attempt.
    PendingAttempt {
        /// Stable current attempt ordinal; inspect the operation for its role/hash.
        attempt: u64,
    },
    /// The endpoint went backwards or disagrees with retained finalized history.
    StaleFinalizedView,
    /// A required historical header or block body is unavailable, not absent.
    HistoryUnavailable {
        /// First height still requiring evidence.
        next: u64,
    },
    /// Inclusion was found, but historical event/receipt decoding is unavailable.
    ReceiptUnavailable,
    /// Complete scanned history has not yet passed mortal validity (or is immortal).
    AwaitingFinality {
        /// Next height to scan.
        next: u64,
        /// Last currently observed finalized height.
        finalized: u64,
    },
    /// This finite recovery call used its historical-block allowance.
    ScanBudget {
        /// Next height to scan on another recovery call.
        next: u64,
    },
}
/// Result of one finite, repeatable recovery call.
#[derive(Debug)]
pub enum RecoveryStatus {
    /// No physical attempt exists, or every earlier attempt is proven unexecuted.
    Rebuildable,
    /// Finalized result decoded with its historical execution metadata.
    Finalized(Box<ExtrinsicReport>),
    /// Work is retained and replacement remains blocked.
    Unresolved(RecoveryReason),
}
impl JournalBinding {
    /// Reconcile up to `maximum_blocks` of finalized history before replacement.
    ///
    /// Missing headers, bodies, receipts or metadata never prove failure. The
    /// submission endpoint is used, as in the existing client's reconciliation.
    /// Neither a spec change nor nonce change makes an attempt unexecuted.
    pub async fn recover(
        &self,
        client: &Client,
        maximum_blocks: u32,
    ) -> Result<RecoveryStatus, JournalError> {
        let record = self.record()?;
        if client.genesis() != record.intent.genesis {
            return Err(JournalError::Identity);
        }
        let shielded = matches!(record.mode, ExecutionMode::Shielded(_));
        match record.finalized_effect() {
            Err(_) => return Ok(RecoveryStatus::Unresolved(RecoveryReason::EvidenceConflict)),
            Ok(Some(attempt)) => return Self::recover_receipt(client, attempt).await,
            Ok(None) => {}
        }
        let mut remaining = maximum_blocks;
        for attempt in record.attempts {
            if attempt.outcome != AttemptOutcome::Unresolved {
                continue;
            }
            if let Some(status) = self
                .recover_attempt(client, attempt, shielded, &mut remaining)
                .await?
            {
                return Ok(status);
            }
        }
        // Another observer may have finalized a different exact attempt while
        // this call was scanning. Never derive a terminal state from its stale
        // initial record or from the last vector element (possibly a carrier).
        let current = self.record()?;
        match current.finalized_effect() {
            Err(_) => Ok(RecoveryStatus::Unresolved(RecoveryReason::EvidenceConflict)),
            Ok(Some(attempt)) => Self::recover_receipt(client, attempt).await,
            Ok(None) if matches!(current.mode, ExecutionMode::Shielded(_)) => Ok(
                RecoveryStatus::Unresolved(RecoveryReason::ShieldAdapterRequired),
            ),
            Ok(None) => match current
                .attempts
                .iter()
                .find(|a| a.outcome != AttemptOutcome::ExpiredUnexecuted)
            {
                Some(attempt) => Ok(RecoveryStatus::Unresolved(RecoveryReason::PendingAttempt {
                    attempt: attempt.id,
                })),
                None => Ok(RecoveryStatus::Rebuildable),
            },
        }
    }

    async fn recover_receipt(
        client: &Client,
        attempt: &SignedAttempt,
    ) -> Result<RecoveryStatus, JournalError> {
        let AttemptOutcome::Finalized {
            block,
            number,
            success,
            ..
        } = attempt.outcome
        else {
            return Err(JournalError::Identity);
        };
        Ok(
            match client.resolve_extrinsic(attempt.hash, block, true).await {
                Ok(report)
                    if report.hash == attempt.hash
                        && report.block_number == Some(number)
                        && report.success == success =>
                {
                    RecoveryStatus::Finalized(Box::new(report))
                }
                Ok(_) => RecoveryStatus::Unresolved(RecoveryReason::EvidenceConflict),
                Err(_) => RecoveryStatus::Unresolved(RecoveryReason::ReceiptUnavailable),
            },
        )
    }

    // A single finite budget is shared across every physical identity. This
    // counts strict historical lookup reads (including repeated target reads),
    // never maximum_blocks again per attempt/pair. Receipt decoding for found
    // targets is additional, at most one such lookup per successful scan here;
    // resolving an already persisted finalized receipt is a separate fast path.
    async fn recover_attempt(
        &self,
        client: &Client,
        mut attempt: SignedAttempt,
        shielded: bool,
        remaining: &mut u32,
    ) -> Result<Option<RecoveryStatus>, JournalError> {
        let chain = match client.submit_client().await {
            Ok(chain) => chain,
            Err(_) => {
                return Ok(Some(RecoveryStatus::Unresolved(
                    RecoveryReason::FinalizedUnavailable,
                )));
            }
        };
        let finalized = match chain.at(crate::BlockRef::Finalized).await {
            Ok(s) => s.block().clone(),
            Err(_) => {
                return Ok(Some(RecoveryStatus::Unresolved(
                    RecoveryReason::FinalizedUnavailable,
                )));
            }
        };
        if let Some((height, hash)) = attempt.finalized {
            if finalized.number < height {
                return Ok(Some(RecoveryStatus::Unresolved(
                    RecoveryReason::StaleFinalizedView,
                )));
            }
            match chain.at(crate::BlockRef::Number(height)).await {
                Ok(s) if s.block().hash == hash => {}
                _ => {
                    return Ok(Some(RecoveryStatus::Unresolved(
                        RecoveryReason::StaleFinalizedView,
                    )));
                }
            }
        }
        while attempt.next <= finalized.number
            && attempt.expiry.is_none_or(|last| attempt.next <= last)
        {
            if *remaining == 0 {
                let next = attempt.next;
                self.save_recovery(attempt).await?;
                return Ok(Some(RecoveryStatus::Unresolved(
                    RecoveryReason::ScanBudget { next },
                )));
            }
            let block = match chain.at(crate::BlockRef::Number(attempt.next)).await {
                Ok(s) => s.block().hash,
                Err(_) => {
                    let next = attempt.next;
                    self.save_recovery(attempt).await?;
                    return Ok(Some(RecoveryStatus::Unresolved(
                        RecoveryReason::HistoryUnavailable { next },
                    )));
                }
            };
            *remaining -= 1;
            match chain.find_extrinsic_complete(attempt.hash, block).await {
                Ok(Some(_)) => {
                    return match client.resolve_extrinsic(attempt.hash, block, true).await {
                        Ok(report) => {
                            self.observed(&report).await?;
                            let current = self.record()?;
                            match current.finalized_effect() {
                                Err(_) => Ok(Some(RecoveryStatus::Unresolved(
                                    RecoveryReason::EvidenceConflict,
                                ))),
                                Ok(Some(effect)) if effect.hash == report.hash => {
                                    Ok(Some(RecoveryStatus::Finalized(Box::new(report))))
                                }
                                // A carrier is only a physical receipt; never
                                // feed it to a logical effect consumer.
                                _ => Ok(None),
                            }
                        }
                        Err(_) => Ok(Some(RecoveryStatus::Unresolved(
                            RecoveryReason::ReceiptUnavailable,
                        ))),
                    };
                }
                Ok(None) => {}
                Err(_) => {
                    let next = attempt.next;
                    self.save_recovery(attempt).await?;
                    return Ok(Some(RecoveryStatus::Unresolved(
                        RecoveryReason::HistoryUnavailable { next },
                    )));
                }
            }
            attempt.next = attempt
                .next
                .checked_add(1)
                .ok_or_else(|| JournalError::Storage("historical cursor exhausted".into()))?;
            attempt.finalized = Some((finalized.number, finalized.hash));
        }
        if attempt
            .expiry
            .is_some_and(|expiry| finalized.number > expiry && attempt.next > expiry)
        {
            // Schema3 is not protocol qualification. In particular a carrier
            // whose inner disappeared must not authorize a new logical effect.
            if !shielded {
                attempt.outcome = AttemptOutcome::ExpiredUnexecuted;
            }
            self.save_recovery(attempt).await?;
            return Ok(None);
        }
        let next = attempt.next;
        self.save_recovery(attempt).await?;
        Ok(Some(RecoveryStatus::Unresolved(
            RecoveryReason::AwaitingFinality {
                next,
                finalized: finalized.number,
            },
        )))
    }

    async fn save_recovery(&self, attempt: SignedAttempt) -> Result<(), JournalError> {
        let binding = self.clone();
        crate::offload(move || {
            binding.journal.update(|image| {
                let current = image
                    .operations
                    .get_mut(&binding.id)
                    .and_then(|r| r.attempts.iter_mut().find(|a| a.id == attempt.id))
                    .ok_or(JournalError::Identity)?;
                if current.hash != attempt.hash || current.role != attempt.role {
                    return Err(JournalError::Conflict);
                }
                // Only progress and outcome can merge. Never replace immutable
                // signed/material evidence with an observer's older snapshot.
                if current.outcome == AttemptOutcome::Unresolved && attempt.next >= current.next {
                    current.next = attempt.next;
                    current.finalized = attempt.finalized;
                }
                graph::merge_outcome(current, attempt.outcome);
                Ok(())
            })
        })
        .await
        .map_err(|_| JournalError::Storage("recovery worker stopped".into()))?
    }
}
