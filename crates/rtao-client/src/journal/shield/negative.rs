//! Versioned, retained negative evidence for the canonical signed-inner route.
//! This is configured finalized-RPC evidence, not independent consensus.
use super::*;
use crate::{BlockHeader, BlockRef, ShieldReport};
use rtao_core::{codec::Value as CoreValue, value_ext::ValueExt};

pub(in crate::journal) const ABSENCE: &str = "rtao_shield_absence_v1";
pub(super) const MAX_PAIRS: u32 = 16;
const MAX_CANONICAL_BYTES: usize = 64 * 1024;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Covered {
    number: u64,
    block: H256,
    parent: H256,
    code: H256,
}
/// Explicit evidence version; not a bearer dispatch permit. Old SDKs preserve
/// the extension but continue refusing shield replacement. Original attempts,
/// signatures, authorization revision and opaque fields are never removed.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::journal) struct Absence {
    version: u32,
    pair: u64,
    generation: u64,
    profile: String,
    inner: H256,
    carrier: H256,
    start: u64,
    end: u64,
    boundary: (u64, H256),
    covered: Vec<Covered>,
}
fn unverified(attempt: u64) -> RecoveryReason {
    RecoveryReason::ShieldProtocolUnverified { attempt }
}
fn unavailable(next: u64) -> RecoveryReason {
    RecoveryReason::HistoryUnavailable { next }
}
impl Absence {
    fn shape(&self, record: &OperationRecord, pair: &ShieldPair) -> Result<(), JournalError> {
        let ExecutionMode::Shielded(policy) = &record.mode else {
            return Err(JournalError::Identity);
        };
        let inner = record.attempt(pair.inner)?;
        let carrier = record.attempt(pair.carrier.ok_or(JournalError::Identity)?)?;
        let end = inner.expiry.ok_or(JournalError::Identity)?;
        if self.version != 1
            || self.pair != pair.id
            || self.generation != pair.material_generation
            || self.profile != policy.protocol
            || self.inner != inner.hash
            || self.carrier != carrier.hash
            || inner.start != carrier.start
            || inner.expiry != carrier.expiry
            || inner.era_anchor != carrier.era_anchor
            || inner.start.checked_add(policy.period) != Some(end)
            || self.start != inner.start
            || self.end != end
            || self.boundary.0 != end.checked_add(1).ok_or(JournalError::Identity)?
            || self.covered.len() as u64 != policy.period + 1
            || self
                .covered
                .first()
                .is_none_or(|first| first.block != inner.era_anchor)
        {
            return Err(JournalError::Identity);
        }
        for (offset, covered) in self.covered.iter().enumerate() {
            if covered.number != self.start + offset as u64
                || (offset > 0 && covered.parent != self.covered[offset - 1].block)
            {
                return Err(JournalError::Identity);
            }
        }
        Ok(())
    }
}
pub(in crate::journal) fn validate_certificate(
    record: &OperationRecord,
    pair: &ShieldPair,
) -> Result<(), JournalError> {
    if let Some(raw) = pair.extensions.get(ABSENCE) {
        let proof: Absence =
            serde_json::from_value(raw.clone()).map_err(|_| JournalError::Identity)?;
        proof.shape(record, pair)?;
        if !matches!(pair.phase, PairPhase::DispatchAuthorized { .. }) {
            return Err(JournalError::Identity);
        }
        let inner = record.attempt(pair.inner)?;
        if inner.outcome != AttemptOutcome::ExpiredUnexecuted
            && !inner
                .evidence_conflicts
                .contains(&AttemptOutcome::ExpiredUnexecuted)
        {
            return Err(JournalError::Identity);
        }
        if record
            .attempt(pair.carrier.ok_or(JournalError::Identity)?)?
            .outcome
            == AttemptOutcome::Unresolved
        {
            return Err(JournalError::Identity);
        }
    }
    Ok(())
}
pub(in crate::journal) fn replacement_proven(record: &OperationRecord, pair: &ShieldPair) -> bool {
    if pair.phase == PairPhase::Retired {
        return true;
    }
    let Some(raw) = pair.extensions.get(ABSENCE) else {
        return false;
    };
    let Ok(proof) = serde_json::from_value::<Absence>(raw.clone()) else {
        return false;
    };
    proof.shape(record, pair).is_ok()
        && record.attempt(pair.inner).is_ok_and(|a| {
            a.outcome == AttemptOutcome::ExpiredUnexecuted && a.evidence_conflicts.is_empty()
        })
        && pair
            .carrier
            .and_then(|id| record.attempt(id).ok())
            .is_some_and(|a| {
                a.outcome != AttemptOutcome::Unresolved && a.evidence_conflicts.is_empty()
            })
}

/// Canonical re-encoding also binds retained unsigned fields and payload to the
/// actual signed bytes. A raw hash/shape or a marker string is insufficient.
fn call_bytes(
    runtime: &rtao_core::runtime::Runtime,
    call: &CoreValue,
    depth: usize,
) -> Option<Vec<u8>> {
    if depth > 256 {
        return None;
    }
    let module = call.field("call_module")?.as_str()?;
    let function = call.field("call_function")?.as_str()?;
    let CoreValue::List(args) = call.field("call_args")? else {
        return None;
    };
    let values = args
        .iter()
        .map(|arg| normalize(runtime, arg.field("value")?, depth + 1))
        .collect::<Option<Vec<_>>>()?;
    runtime
        .compose_call(module, function, &CoreValue::List(values))
        .ok()
}
// The pinned codec deliberately decodes calls into inspection dictionaries,
// while its encoder takes composed bytes or enum input. Recompose those
// dictionaries with the existing metadata encoder, including nested calls;
// equality of the complete signed envelope remains mandatory.
fn normalize(
    runtime: &rtao_core::runtime::Runtime,
    value: &CoreValue,
    depth: usize,
) -> Option<CoreValue> {
    if depth > 256 {
        return None;
    }
    Some(match value {
        CoreValue::Dict(entries)
            if entries.len() == 5
                && value.field("call_index").is_some()
                && value.field("call_hash").is_some() =>
        {
            CoreValue::Bytes(call_bytes(runtime, value, depth + 1)?)
        }
        CoreValue::Dict(entries) => CoreValue::Dict(
            entries
                .iter()
                .map(|(k, v)| {
                    Some((
                        normalize(runtime, k, depth + 1)?,
                        normalize(runtime, v, depth + 1)?,
                    ))
                })
                .collect::<Option<_>>()?,
        ),
        CoreValue::List(items) => CoreValue::List(
            items
                .iter()
                .map(|v| normalize(runtime, v, depth + 1))
                .collect::<Option<_>>()?,
        ),
        CoreValue::Tuple(items) => CoreValue::Tuple(
            items
                .iter()
                .map(|v| normalize(runtime, v, depth + 1))
                .collect::<Option<_>>()?,
        ),
        other => other.clone(),
    })
}
fn canonical(
    runtime: &rtao_core::runtime::Runtime,
    attempt: &SignedAttempt,
    account: AccountId,
    genesis: H256,
    period: u64,
) -> Result<(), RecoveryReason> {
    let refuse = || RecoveryReason::ShieldCanonicalEncodingRequired {
        attempt: attempt.id,
    };
    let decoded = runtime
        .decode_extrinsic(&attempt.bytes, true)
        .map_err(|_| refuse())?;
    if matches!(attempt.role, AttemptRole::Carrier { .. }) {
        let call = decoded.field("call").ok_or_else(refuse)?;
        if call.field("call_module").and_then(CoreValue::as_str) != Some("MevShield")
            || call.field("call_function").and_then(CoreValue::as_str) != Some("submit_encrypted")
        {
            return Err(unverified(attempt.id));
        }
    }
    let signature = decoded.field("signature").ok_or_else(refuse)?;
    let (crypto, signature) = if let Some(raw) = signature.variant("Ed25519") {
        (0, raw.value_bytes().ok_or_else(refuse)?)
    } else if let Some(raw) = signature.variant("Sr25519") {
        (1, raw.value_bytes().ok_or_else(refuse)?)
    } else {
        return Err(refuse());
    };
    let address = decoded.field("address").ok_or_else(refuse)?;
    let parsed = address
        .as_str()
        .and_then(|text| text.parse::<AccountId>().ok())
        .or_else(|| {
            address
                .value_bytes()
                .and_then(|b| b.try_into().ok())
                .map(AccountId)
        });
    if parsed != Some(account) {
        return Err(refuse());
    }
    let era = decoded.field("era").ok_or_else(refuse)?;
    let read = |i| {
        era.tuple_item(i)
            .and_then(CoreValue::as_u128)
            .and_then(|v| u64::try_from(v).ok())
            .ok_or_else(refuse)
    };
    if read(0)? != period
        || read(1)? != attempt.start % period
        || attempt.expiry != attempt.start.checked_add(period)
        || attempt.era.get("period").and_then(Value::as_u64) != Some(period)
        || attempt.era.get("current").and_then(Value::as_u64) != Some(attempt.start)
    {
        return Err(refuse());
    }
    let era = CoreValue::record(vec![
        ("period".into(), CoreValue::Uint(period.into())),
        ("current".into(), CoreValue::Uint(attempt.start.into())),
    ]);
    let tip = attempt
        .extensions
        .get("tip_rao")
        .and_then(Value::as_u64)
        .ok_or_else(refuse)?;
    let asset = attempt.extensions.get("tip_asset_id").ok_or_else(refuse)?;
    let asset = if asset.is_null() {
        None
    } else {
        Some(asset.as_u64().ok_or_else(refuse)?.into())
    };
    let params = rtao_core::extrinsic::TxParams {
        era,
        nonce: attempt.nonce,
        tip: tip.into(),
        tip_asset_id: asset,
        genesis_hash: genesis.0,
        era_block_hash: attempt.era_anchor.0,
        metadata_hash: attempt.metadata_hash,
    };
    let call =
        call_bytes(runtime, decoded.field("call").ok_or_else(refuse)?, 0).ok_or_else(refuse)?;
    let (bytes, hash) = runtime
        .encode_signed_extrinsic(&call, account.0, &signature, crypto, &params)
        .map_err(|_| refuse())?;
    let payload = runtime
        .signature_payload(&call, &params)
        .map_err(|_| refuse())?;
    if bytes != attempt.bytes
        || hash != attempt.hash.0
        || attempt.extensions.get("signing_payload_hash")
            != Some(&serde_json::json!(digest(&payload)))
        || !rtao_core::keys::verify_strict(&payload, &signature, &account.to_ss58(42), crypto)
            .map_err(|_| refuse())?
    {
        return Err(refuse());
    }
    Ok(())
}

impl JournalBinding {
    async fn retain_code_refusal(
        &self,
        mut reason: Incompatibility,
        state: H256,
        code: H256,
    ) -> Result<(), JournalError> {
        reason
            .extensions
            .entry("rtao_runtime_state".into())
            .or_insert(serde_json::json!(state));
        reason
            .extensions
            .entry("rtao_runtime_code".into())
            .or_insert(serde_json::json!(code));
        let binding = self.clone();
        crate::offload(move || {
            binding.journal.update(|image| {
                image
                    .operations
                    .get_mut(&binding.id)
                    .ok_or(JournalError::Identity)?
                    .incompatibility = Some(reason);
                Ok(())
            })
        })
        .await
        .map_err(|_| JournalError::Storage("shield incompatibility worker stopped".into()))?
    }
    async fn historical_header(
        client: &Client,
        number: u64,
    ) -> Result<BlockHeader, RecoveryReason> {
        let hash: Value = client
            .request("chain_getBlockHash", serde_json::json!([number]))
            .await
            .map_err(|_| unavailable(number))?;
        let hash = crate::parse_hash(&hash).map_err(|_| unavailable(number))?;
        let raw: Value = client
            .request(
                "chain_getHeader",
                serde_json::json!([crate::hash_hex(hash)]),
            )
            .await
            .map_err(|_| unavailable(number))?;
        let header =
            BlockHeader::from_json_with_hash(raw, hash).map_err(|_| unavailable(number))?;
        if header.number != number {
            return Err(unavailable(number));
        }
        Ok(header)
    }
    async fn check_retained_finality(
        client: &Client,
        record: &OperationRecord,
        tip: &crate::PinnedBlock,
    ) -> Result<(), RecoveryReason> {
        for attempt in &record.attempts {
            if let Some((number, hash)) = attempt.finalized
                && (tip.number < number
                    || Self::historical_header(client, number).await?.hash != hash)
            {
                return Err(RecoveryReason::StaleFinalizedView);
            }
        }
        Ok(())
    }
    async fn qualify_component(
        &self,
        chain: &Client,
        record: &OperationRecord,
        pair: &ShieldPair,
        attempt: &SignedAttempt,
        field: &str,
        period: u64,
    ) -> Result<(), RecoveryReason> {
        if attempt.bytes.len() > MAX_CANONICAL_BYTES {
            return Err(RecoveryReason::ShieldCanonicalEncodingRequired {
                attempt: attempt.id,
            });
        }
        let context = pair
            .extensions
            .get(field)
            .ok_or_else(|| unverified(attempt.id))?;
        let state: H256 = serde_json::from_value(
            context
                .get("state")
                .cloned()
                .ok_or_else(|| unverified(attempt.id))?,
        )
        .map_err(|_| unverified(attempt.id))?;
        let qualified = self
            .qualify_shield_runtime(chain, state)
            .await
            .map_err(|_| unverified(attempt.id))?;
        if qualified.evidence() != *context
            || qualified.spec_version != attempt.spec_version
            || qualified.transaction_version != attempt.transaction_version
            || attempt.extensions.get("signing_metadata_hash")
                != Some(&serde_json::json!(qualified.metadata_hash))
        {
            return Err(unverified(attempt.id));
        }
        let metadata = chain
            .fresh_signing_metadata(state)
            .await
            .map_err(|_| unverified(attempt.id))?;
        if digest(&metadata) != qualified.metadata_hash {
            return Err(unverified(attempt.id));
        }
        let attempt = attempt.clone();
        let account = record.intent.account;
        let genesis = record.intent.genesis;
        crate::offload(move || {
            let runtime = rtao_core::runtime::Runtime::parse(
                &metadata,
                attempt.spec_version,
                attempt.transaction_version,
                42,
            )
            .map_err(|_| unverified(attempt.id))?;
            canonical(&runtime, &attempt, account, genesis, period)
        })
        .await
        .map_err(|_| unverified(pair.inner))?
    }
    /// All old cursors are deliberately ignored for negative qualification.
    /// One finite body allowance covers every pair and both physical hashes.
    pub(in crate::journal) async fn recover_shield_absence(
        &self,
        client: &Client,
        remaining: &mut u32,
    ) -> Result<RecoveryStatus, JournalError> {
        let record = self.record()?;
        let ExecutionMode::Shielded(policy) = &record.mode else {
            return Err(JournalError::Identity);
        };
        if record.shield_pairs.len() > MAX_PAIRS as usize {
            return Ok(RecoveryStatus::Unresolved(
                RecoveryReason::ShieldWorkLimit {
                    maximum_pairs: MAX_PAIRS,
                },
            ));
        }
        let Some(guard) = &self.protocol_guard else {
            return Ok(RecoveryStatus::Unresolved(
                RecoveryReason::ShieldAdapterRequired,
            ));
        };
        if guard.profile() != policy.protocol {
            return Ok(RecoveryStatus::Unresolved(unverified(0)));
        }
        let chain = match client.submit_client().await {
            Ok(c) => c,
            Err(_) => {
                return Ok(RecoveryStatus::Unresolved(
                    RecoveryReason::FinalizedUnavailable,
                ));
            }
        };
        let tip = match chain.at(BlockRef::Finalized).await {
            Ok(s) => s.block().clone(),
            Err(_) => {
                return Ok(RecoveryStatus::Unresolved(
                    RecoveryReason::FinalizedUnavailable,
                ));
            }
        };
        if let Err(reason) = Self::check_retained_finality(&chain, &record, &tip).await {
            return Ok(RecoveryStatus::Unresolved(reason));
        }
        for pair in &record.shield_pairs {
            if !matches!(pair.phase, PairPhase::DispatchAuthorized { .. }) {
                continue;
            }
            if let Some(raw) = pair.extensions.get(ABSENCE) {
                let proof: Absence =
                    serde_json::from_value(raw.clone()).map_err(|_| JournalError::Identity)?;
                proof.shape(&record, pair)?;
                if tip.number < proof.boundary.0 {
                    return Ok(RecoveryStatus::Unresolved(
                        RecoveryReason::StaleFinalizedView,
                    ));
                }
                match Self::historical_header(&chain, proof.boundary.0).await {
                    Ok(header) if header.hash == proof.boundary.1 => {}
                    Ok(_) => {
                        return Ok(RecoveryStatus::Unresolved(
                            RecoveryReason::StaleFinalizedView,
                        ));
                    }
                    Err(reason) => return Ok(RecoveryStatus::Unresolved(reason)),
                }
                for covered in &proof.covered {
                    if let Err(reason) = guard.check(record.intent.genesis, covered.code) {
                        self.retain_code_refusal(reason, covered.parent, covered.code)
                            .await?;
                        return Ok(RecoveryStatus::Unresolved(unverified(pair.inner)));
                    }
                }
                for field in ["signing_context", "carrier_context"] {
                    let code = pair
                        .extensions
                        .get(field)
                        .and_then(|v| v.get("runtime_code_hash"))
                        .cloned()
                        .and_then(|v| serde_json::from_value::<H256>(v).ok());
                    if code.is_none_or(|code| guard.check(record.intent.genesis, code).is_err()) {
                        return Ok(RecoveryStatus::Unresolved(unverified(pair.inner)));
                    }
                }
                continue;
            }
            let inner = record.attempt(pair.inner)?;
            let carrier = record.attempt(pair.carrier.ok_or(JournalError::Identity)?)?;
            let Some(end) = inner.expiry else {
                return Ok(RecoveryStatus::Unresolved(unverified(inner.id)));
            };
            if inner.start != carrier.start
                || inner.expiry != carrier.expiry
                || inner.era_anchor != carrier.era_anchor
                || inner.start.checked_add(policy.period) != Some(end)
            {
                return Ok(RecoveryStatus::Unresolved(unverified(inner.id)));
            }
            if tip.number <= end {
                return Ok(RecoveryStatus::Unresolved(
                    RecoveryReason::AwaitingFinality {
                        next: inner.start,
                        finalized: tip.number,
                    },
                ));
            }
            for (attempt, field) in [(inner, "signing_context"), (carrier, "carrier_context")] {
                if let Err(reason) = self
                    .qualify_component(&chain, &record, pair, attempt, field, policy.period)
                    .await
                {
                    return Ok(RecoveryStatus::Unresolved(reason));
                }
                match Self::historical_header(&chain, attempt.start).await {
                    Ok(header) if header.hash == attempt.era_anchor => {}
                    Ok(_) => {
                        return Ok(RecoveryStatus::Unresolved(
                            RecoveryReason::ShieldEraAnchorMismatch {
                                attempt: attempt.id,
                            },
                        ));
                    }
                    Err(reason) => return Ok(RecoveryStatus::Unresolved(reason)),
                }
            }
            // Small exact SDK windows (4/8) permit a full atomic certificate.
            // A smaller call budget reports the requirement without partial
            // qualification; it cannot borrow unqualified cursor coverage.
            let needed = u32::try_from(policy.period + 1).map_err(|_| JournalError::Identity)?;
            if *remaining < needed {
                return Ok(RecoveryStatus::Unresolved(
                    RecoveryReason::ShieldQualificationBudget { required: needed },
                ));
            }
            let mut covered: Vec<Covered> = Vec::new();
            for number in inner.start..=end {
                let header = match Self::historical_header(&chain, number).await {
                    Ok(h) => h,
                    Err(r) => return Ok(RecoveryStatus::Unresolved(r)),
                };
                if number == inner.start && header.hash != inner.era_anchor {
                    return Ok(RecoveryStatus::Unresolved(
                        RecoveryReason::ShieldEraAnchorMismatch { attempt: inner.id },
                    ));
                }
                let Some(parent) = header.parent_hash else {
                    return Ok(RecoveryStatus::Unresolved(unavailable(number)));
                };
                if covered.last().is_some_and(|last| last.block != parent) {
                    return Ok(RecoveryStatus::Unresolved(
                        RecoveryReason::StaleFinalizedView,
                    ));
                }
                let code: Value = match chain
                    .request(
                        "state_getStorageHash",
                        serde_json::json!(["0x3a636f6465", crate::hash_hex(parent)]),
                    )
                    .await
                {
                    Ok(v) => v,
                    Err(_) => return Ok(RecoveryStatus::Unresolved(unverified(inner.id))),
                };
                let code = match crate::parse_hash(&code) {
                    Ok(v) => v,
                    Err(_) => return Ok(RecoveryStatus::Unresolved(unverified(inner.id))),
                };
                if let Err(reason) = guard.check(record.intent.genesis, code) {
                    self.retain_code_refusal(reason, parent, code).await?;
                    return Ok(RecoveryStatus::Unresolved(unverified(inner.id)));
                }
                *remaining -= 1;
                let raw: Value = match chain
                    .request(
                        "chain_getBlock",
                        serde_json::json!([crate::hash_hex(header.hash)]),
                    )
                    .await
                {
                    Ok(v) => v,
                    Err(_) => return Ok(RecoveryStatus::Unresolved(unavailable(number))),
                };
                let Some(raw_header) = raw.pointer("/block/header") else {
                    return Ok(RecoveryStatus::Unresolved(unavailable(number)));
                };
                let valid_header =
                    BlockHeader::from_json_with_hash(raw_header.clone(), header.hash).is_ok_and(
                        |body| body.number == number && body.parent_hash == Some(parent),
                    );
                if !valid_header
                    || raw
                        .pointer("/block/extrinsics")
                        .and_then(Value::as_array)
                        .is_none()
                {
                    return Ok(RecoveryStatus::Unresolved(unavailable(number)));
                }
                for attempt in [inner, carrier] {
                    let found = match rtao_core::ops::extrinsic_lookup::find(&raw, attempt.hash.0) {
                        Ok(v) => v,
                        Err(_) => return Ok(RecoveryStatus::Unresolved(unavailable(number))),
                    };
                    if found.is_some() {
                        let current = self.record()?;
                        if current.finalized_effect().is_err() {
                            return Ok(RecoveryStatus::Unresolved(
                                RecoveryReason::EvidenceConflict,
                            ));
                        }
                        if attempt.id == carrier.id
                            && matches!(current.attempt(attempt.id)?.outcome,
                            AttemptOutcome::Finalized {block,number:height,..} if block==header.hash && height==number)
                        {
                            continue;
                        }
                        let mut report = match client
                            .resolve_extrinsic(attempt.hash, header.hash, true)
                            .await
                        {
                            Ok(r) => r,
                            Err(_) => {
                                return Ok(RecoveryStatus::Unresolved(
                                    RecoveryReason::ReceiptUnavailable,
                                ));
                            }
                        };
                        if report.hash != attempt.hash
                            || report.block_hash != Some(header.hash)
                            || report.block_number != Some(number)
                            || report.inclusion != crate::Inclusion::Finalized
                        {
                            return Ok(RecoveryStatus::Unresolved(
                                RecoveryReason::EvidenceConflict,
                            ));
                        }
                        self.observed(&report).await?;
                        let current = self.record()?;
                        if current.finalized_effect().is_err() {
                            return Ok(RecoveryStatus::Unresolved(
                                RecoveryReason::EvidenceConflict,
                            ));
                        }
                        if attempt.id == inner.id {
                            report.shield = Some(ShieldReport {
                                carrier_hash: carrier.hash,
                                inner_hash: inner.hash,
                            });
                            return Ok(RecoveryStatus::Finalized(Box::new(report)));
                        }
                    }
                }
                covered.push(Covered {
                    number,
                    block: header.hash,
                    parent,
                    code,
                });
            }
            let boundary = match Self::historical_header(&chain, end + 1).await {
                Ok(h) => h,
                Err(r) => return Ok(RecoveryStatus::Unresolved(r)),
            };
            if boundary.parent_hash != covered.last().map(|h| h.block) {
                return Ok(RecoveryStatus::Unresolved(
                    RecoveryReason::StaleFinalizedView,
                ));
            }
            let proof = Absence {
                version: 1,
                pair: pair.id,
                generation: pair.material_generation,
                profile: policy.protocol.clone(),
                inner: inner.hash,
                carrier: carrier.hash,
                start: inner.start,
                end,
                boundary: (boundary.number, boundary.hash),
                covered,
            };
            proof.shape(&record, pair)?;
            let raw = serde_json::to_value(&proof).map_err(|_| JournalError::Identity)?;
            let binding = self.clone();
            let pair_id = pair.id;
            let length = record.shield_pairs.len();
            crate::offload(move || {
                binding.journal.update(|image| {
                    let current = image
                        .operations
                        .get_mut(&binding.id)
                        .ok_or(JournalError::Identity)?;
                    if current.shield_pairs.len() != length {
                        return Err(JournalError::Conflict);
                    }
                    let group = current
                        .shield_pairs
                        .get(pair_id as usize)
                        .ok_or(JournalError::Identity)?;
                    proof.shape(current, group)?;
                    if group.extensions.contains_key(ABSENCE) {
                        return Err(JournalError::Conflict);
                    }
                    graph::merge_outcome(
                        &mut current.attempts[group.inner as usize],
                        AttemptOutcome::ExpiredUnexecuted,
                    );
                    let carrier = group.carrier.ok_or(JournalError::Identity)? as usize;
                    if current.attempts[carrier].outcome == AttemptOutcome::Unresolved {
                        graph::merge_outcome(
                            &mut current.attempts[carrier],
                            AttemptOutcome::ExpiredUnexecuted,
                        );
                    }
                    current.shield_pairs[pair_id as usize]
                        .extensions
                        .insert(ABSENCE.into(), raw);
                    Ok(())
                })
            })
            .await
            .map_err(|_| {
                JournalError::Storage("negative shield evidence worker stopped".into())
            })??;
        }
        let current = self.record()?;
        match current.finalized_effect() {
            Err(_) => Ok(RecoveryStatus::Unresolved(RecoveryReason::EvidenceConflict)),
            Ok(Some(attempt)) => {
                Self::recover_receipt(client, attempt, current.effect_shield(attempt)?).await
            }
            Ok(None)
                if current.shield_pairs.iter().all(|p| {
                    !matches!(p.phase, PairPhase::DispatchAuthorized { .. })
                        || replacement_proven(&current, p)
                }) =>
            {
                if current.shield_pairs.len() as u64 >= u64::from(policy.maximum_pairs) {
                    Ok(RecoveryStatus::Unresolved(
                        RecoveryReason::ShieldPairBudgetExhausted {
                            maximum_pairs: policy.maximum_pairs,
                            consumed: u32::try_from(current.shield_pairs.len())
                                .map_err(|_| JournalError::Identity)?,
                        },
                    ))
                } else {
                    Ok(RecoveryStatus::Rebuildable)
                }
            }
            _ => Ok(RecoveryStatus::Unresolved(RecoveryReason::InnerUnresolved)),
        }
    }
}
