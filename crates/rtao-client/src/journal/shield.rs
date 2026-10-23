//! Paired preparation/ownership in the existing operation journal.
use super::*;
mod negative;
pub(in crate::journal) use negative::{ABSENCE, replacement_proven, validate_certificate};

/// Trusted adapter qualification for native signed-inner execution semantics.
///
/// The SDK obtains actual code hashes at pinned states from the configured
/// history/submission RPC. This guard must match those hashes against trusted
/// independently audited code identities, never a peer-provided claim, spec
/// version alone, or metadata shape alone. It does not establish application
/// economics (SemanticGuard does that) or independent consensus/RPC honesty.
pub trait ShieldProtocolGuard: Send + Sync {
    /// Stable audited signed-inner protocol identity, retained across restart.
    fn profile(&self) -> &str;
    /// Qualify this chain and exact runtime code for the audited semantics.
    fn check(&self, genesis: H256, code_hash: H256) -> Result<(), Incompatibility>;
}

pub(crate) struct ShieldPreparation {
    binding: JournalBinding,
    claims: Arc<super::ownership::SubmissionClaims>,
    pub(crate) policy: ShieldPolicy,
}
/// Qualified context is minted only after an installed trusted guard examines
/// the actual pinned state's code identity, not from an RPC profile string.
pub(crate) struct QualifiedShieldRuntime {
    profile: String,
    code_hash: H256,
    state: H256,
    spec_version: u32,
    transaction_version: u32,
    metadata_hash: H256,
}

impl JournalBinding {
    /// Inspect journal readiness for shield preparation on the selected route.
    /// Checks the retained protocol, period, original pair budget, material,
    /// pause and qualified reconciliation evidence without changing the graph.
    /// Runtime/semantic qualification, ownership and actual submission policy
    /// are still checked by preparation; this result grants no author authority.
    pub fn can_prepare_shield(&self, period: crate::ShieldPeriod) -> Result<(), JournalError> {
        let image = self.journal.snapshot()?;
        let record = image
            .operations
            .get(&self.id)
            .ok_or(JournalError::Identity)?;
        if record.material_generation != self.material_generation {
            return Err(JournalError::Conflict);
        }
        let guard = self.protocol_guard.as_ref().ok_or_else(|| {
            JournalError::Blocked("reinstall trusted shield runtime qualification".into())
        })?;
        if guard.profile().is_empty() || guard.profile().len() > 256 {
            return Err(JournalError::Identity);
        }
        match &record.mode {
            ExecutionMode::Unselected => {
                if image.control.submissions_paused {
                    return Err(JournalError::Blocked(
                        "new shield authorization paused".into(),
                    ));
                }
                dispatchable(record, &image.control)
            }
            ExecutionMode::Shielded(policy) => {
                if policy.protocol != guard.profile() || policy.period != period.get() {
                    return Err(JournalError::Identity);
                }
                admission(record, self, policy, &image.control)?;
                if record.shield_pairs.len() as u64 >= u64::from(policy.maximum_pairs) {
                    return Err(JournalError::Blocked(
                        "original shield pair budget exhausted".into(),
                    ));
                }
                Ok(())
            }
            ExecutionMode::Ordinary => Err(JournalError::Identity),
        }
    }
    pub(crate) async fn qualify_shield_runtime(
        &self,
        client: &Client,
        state: H256,
    ) -> Result<QualifiedShieldRuntime, JournalError> {
        let record = self.record()?;
        if client.genesis() != record.intent.genesis {
            return Err(JournalError::Identity);
        }
        let guard = self.protocol_guard.as_ref().ok_or_else(|| {
            JournalError::Blocked("reinstall trusted shield runtime qualification".into())
        })?;
        if guard.profile().is_empty() || guard.profile().len() > 256 {
            return Err(JournalError::Identity);
        }
        let code: Value = client
            .request(
                "state_getStorageHash",
                serde_json::json!(["0x3a636f6465", crate::hash_hex(state)]),
            )
            .await
            .map_err(|_| {
                JournalError::Blocked("pinned runtime code identity unavailable".into())
            })?;
        let code_hash = crate::parse_hash(&code).map_err(|_| {
            JournalError::Blocked("pinned runtime code identity unavailable".into())
        })?;
        if let Err(mut reason) = guard.check(client.genesis(), code_hash) {
            reason
                .extensions
                .entry("rtao_runtime_state".into())
                .or_insert(serde_json::json!(state));
            reason
                .extensions
                .entry("rtao_runtime_code".into())
                .or_insert(serde_json::json!(code_hash));
            self.journal.update(|image| {
                image
                    .operations
                    .get_mut(&self.id)
                    .ok_or(JournalError::Identity)?
                    .incompatibility = Some(reason.clone());
                Ok(())
            })?;
            return Err(JournalError::Blocked(
                "shield runtime protocol is not qualified".into(),
            ));
        }
        let version: Value = client
            .request(
                "state_getRuntimeVersion",
                serde_json::json!([crate::hash_hex(state)]),
            )
            .await
            .map_err(|_| JournalError::Blocked("pinned runtime version unavailable".into()))?;
        let number = |name: &str| {
            version
                .get(name)
                .and_then(Value::as_u64)
                .and_then(|n| u32::try_from(n).ok())
                .ok_or(JournalError::Identity)
        };
        let metadata = client.fresh_signing_metadata(state).await.map_err(|_| {
            JournalError::Blocked("fresh pinned signing metadata unavailable".into())
        })?;
        Ok(QualifiedShieldRuntime {
            metadata_hash: digest(&metadata),
            profile: guard.profile().to_owned(),
            code_hash,
            state,
            spec_version: number("specVersion")?,
            transaction_version: number("transactionVersion")?,
        })
    }

    pub(crate) async fn qualify_shield_head(
        &self,
        client: &Client,
    ) -> Result<QualifiedShieldRuntime, JournalError> {
        let chain = client
            .submit_client()
            .await
            .map_err(|_| JournalError::Blocked("submission endpoint unavailable".into()))?;
        chain.native_author_session().await.map_err(|_| {
            JournalError::Blocked("durable shield requires native author custody".into())
        })?;
        let best: Value = chain
            .request("chain_getBlockHash", serde_json::json!([null]))
            .await
            .map_err(|_| JournalError::Blocked("fresh submission head unavailable".into()))?;
        let state = crate::parse_hash(&best).map_err(|_| JournalError::Identity)?;
        self.qualify_shield_runtime(&chain, state).await
    }
    pub(crate) fn shield_policy(
        &self,
        period: u64,
        retries: u32,
        maximum_inner_fee: Option<u64>,
    ) -> Result<ShieldPolicy, JournalError> {
        if retries >= negative::MAX_PAIRS {
            return Err(JournalError::Blocked(
                "shield supports at most 16 total pairs".into(),
            ));
        }
        let guard = self.protocol_guard.as_ref().ok_or_else(|| {
            JournalError::Blocked("reinstall trusted shield runtime qualification".into())
        })?;
        Ok(ShieldPolicy {
            protocol: guard.profile().to_owned(),
            period,
            maximum_pairs: retries
                .checked_add(1)
                .ok_or_else(|| JournalError::Blocked("shield pair budget overflow".into()))?,
            extensions: BTreeMap::from([
                ("policy_version".into(), serde_json::json!(1)),
                (
                    "maximum_inner_fee_rao".into(),
                    serde_json::json!(maximum_inner_fee),
                ),
            ]),
        })
    }
    pub(crate) fn claim_shield(
        &self,
        policy: ShieldPolicy,
    ) -> Result<ShieldPreparation, JournalError> {
        let claims = self.claim_submission()?;
        self.journal.update(|image| {
            let current = image
                .operations
                .get_mut(&self.id)
                .ok_or(JournalError::Identity)?;
            if current.material_generation != self.material_generation {
                return Err(JournalError::Conflict);
            }
            match &current.mode {
                ExecutionMode::Unselected => {
                    if image.control.submissions_paused {
                        return Err(JournalError::Blocked(
                            "new shield authorization paused".into(),
                        ));
                    }
                    current.mode = ExecutionMode::Shielded(policy.clone());
                }
                ExecutionMode::Shielded(previous) if previous == &policy => {}
                _ => return Err(JournalError::Identity),
            }
            if image.control.submissions_paused
                && !(image.control.allow_obligations && current.intent.obligation)
            {
                return Err(JournalError::Blocked("shield preparation paused".into()));
            }
            if current.attempts.iter().any(|a| {
                !a.evidence_conflicts.is_empty()
                    || (matches!(a.outcome, AttemptOutcome::Finalized { .. })
                        && !matches!(a.role, AttemptRole::Carrier { .. }))
            }) {
                return Err(JournalError::Blocked(
                    "shield effect or conflicting evidence already retained".into(),
                ));
            }
            for pair in &mut current.shield_pairs {
                match pair.phase {
                    PairPhase::Preparing | PairPhase::Prepared => pair.phase = PairPhase::Retired,
                    PairPhase::Retired => {}
                    PairPhase::DispatchAuthorized { .. } => {
                        // Checked below against the complete immutable graph;
                        // the authorized phase/identity is never erased.
                    }
                }
            }
            if current
                .shield_pairs
                .iter()
                .any(|p| !replacement_proven(current, p))
            {
                return Err(JournalError::Blocked(
                    "authorized shield pair requires finalized reconciliation".into(),
                ));
            }
            if current.shield_pairs.len() as u64 >= u64::from(policy.maximum_pairs) {
                return Err(JournalError::Blocked(
                    "original shield pair budget exhausted".into(),
                ));
            }
            Ok(())
        })?;
        Ok(ShieldPreparation {
            binding: self.clone(),
            claims,
            policy,
        })
    }
}

pub(super) fn digest(bytes: &[u8]) -> H256 {
    use blake2::{Blake2b, Digest, digest::consts::U32};
    H256(Blake2b::<U32>::digest(bytes).into())
}
impl QualifiedShieldRuntime {
    fn check(
        &self,
        policy: &ShieldPolicy,
        unsigned: &UnsignedExtrinsic,
    ) -> Result<(), JournalError> {
        let stamp = unsigned.call().stamp();
        if self.profile != policy.protocol
            || self.spec_version != stamp.spec_version
            || self.transaction_version != stamp.transaction_version
            || self.metadata_hash != digest(&unsigned.context().metadata_bytes)
        {
            return Err(JournalError::Blocked(
                "prepared shield signing context changed; rebuild before dispatch".into(),
            ));
        }
        Ok(())
    }
    fn evidence(&self) -> Value {
        serde_json::json!({"profile":self.profile,"runtime_code_hash":self.code_hash,"state":self.state,
            "spec_version":self.spec_version,"transaction_version":self.transaction_version,"metadata_hash":self.metadata_hash})
    }
}
fn admission(
    record: &OperationRecord,
    binding: &JournalBinding,
    policy: &ShieldPolicy,
    control: &JournalControl,
) -> Result<(), JournalError> {
    if record.material_generation != binding.material_generation {
        return Err(JournalError::Conflict);
    }
    if record.mode != ExecutionMode::Shielded(policy.clone()) {
        return Err(JournalError::Identity);
    }
    if control.submissions_paused && !(control.allow_obligations && record.intent.obligation) {
        return Err(JournalError::Blocked(
            "shield authorization paused; observation continues".into(),
        ));
    }
    if record.attempts.iter().any(|a| {
        !a.evidence_conflicts.is_empty()
            || (matches!(a.outcome, AttemptOutcome::Finalized { .. })
                && !matches!(a.role, AttemptRole::Carrier { .. }))
    }) {
        return Err(JournalError::Blocked("conflicting shield evidence".into()));
    }
    if record.shield_pairs.iter().any(|p| {
        matches!(p.phase, PairPhase::DispatchAuthorized { .. }) && !replacement_proven(record, p)
    }) {
        return Err(JournalError::Blocked(
            "prior shield pair needs qualified reconciliation".into(),
        ));
    }
    Ok(())
}
impl ShieldPreparation {
    pub(crate) async fn check_inner(
        &self,
        client: &Client,
        inner: &UnsignedExtrinsic,
        context: &QualifiedShieldRuntime,
    ) -> Result<(), JournalError> {
        context.check(&self.policy, inner)?;
        self.binding.check_context(client, inner).await?;
        let image = self.binding.journal.snapshot()?;
        admission(
            image
                .operations
                .get(&self.binding.id)
                .ok_or(JournalError::Identity)?,
            &self.binding,
            &self.policy,
            &image.control,
        )
    }
    pub(crate) async fn publish_inner(
        &self,
        unsigned: &UnsignedExtrinsic,
        bytes: &[u8],
        hash: H256,
        context: &QualifiedShieldRuntime,
    ) -> Result<u64, JournalError> {
        context.check(&self.policy, unsigned)?;
        let claims = self.claims.clone();
        let binding = self.binding.clone();
        let policy = self.policy.clone();
        let evidence = context.evidence();
        let mut attempt = super::signed_attempt(
            unsigned,
            bytes,
            hash,
            AttemptRole::Ordinary,
            binding.material_generation,
        );
        binding.attach_submission_context(&mut attempt);
        crate::offload(move || {
            let _claims = claims;
            binding.journal.update(|image| {
                let record = image
                    .operations
                    .get_mut(&binding.id)
                    .ok_or(JournalError::Identity)?;
                admission(record, &binding, &policy, &image.control)?;
                if let Some(previous) = record.attempts.iter().find(|a| a.hash == attempt.hash) {
                    return Err(JournalError::FreshSigningContext {
                        attempt: previous.id,
                    });
                }
                if record.shield_pairs.len() as u64 >= u64::from(policy.maximum_pairs)
                    || record
                        .shield_pairs
                        .iter()
                        .any(|p| !replacement_proven(record, p))
                {
                    return Err(JournalError::Blocked(
                        "shield pair budget or pending pair prevents preparation".into(),
                    ));
                }
                let pair =
                    u64::try_from(record.shield_pairs.len()).map_err(|_| JournalError::Identity)?;
                attempt.id =
                    u64::try_from(record.attempts.len()).map_err(|_| JournalError::Identity)?;
                attempt.role = AttemptRole::Inner { pair };
                attempt.material = record.material.clone();
                record.shield_pairs.push(ShieldPair {
                    id: pair,
                    material_generation: binding.material_generation,
                    inner: attempt.id,
                    carrier: None,
                    phase: PairPhase::Preparing,
                    extensions: BTreeMap::from([("signing_context".into(), evidence)]),
                });
                record.attempts.push(attempt);
                Ok(pair)
            })
        })
        .await
        .map_err(|_| JournalError::Storage("shield publication worker stopped".into()))?
    }
    pub(crate) async fn publish_carrier(
        &self,
        pair: u64,
        unsigned: &UnsignedExtrinsic,
        bytes: &[u8],
        hash: H256,
        sealing_key: &[u8],
        context: &QualifiedShieldRuntime,
    ) -> Result<(), JournalError> {
        context.check(&self.policy, unsigned)?;
        let claims = self.claims.clone();
        let binding = self.binding.clone();
        let policy = self.policy.clone();
        let evidence = context.evidence();
        let key_hash = digest(sealing_key);
        let mut attempt = super::signed_attempt(
            unsigned,
            bytes,
            hash,
            AttemptRole::Carrier { pair },
            binding.material_generation,
        );
        binding.attach_submission_context(&mut attempt);
        crate::offload(move || {
            let _claims = claims;
            binding.journal.update(|image| {
                let record = image
                    .operations
                    .get_mut(&binding.id)
                    .ok_or(JournalError::Identity)?;
                admission(record, &binding, &policy, &image.control)?;
                let index = usize::try_from(pair).map_err(|_| JournalError::Identity)?;
                let group = record
                    .shield_pairs
                    .get(index)
                    .ok_or(JournalError::Identity)?;
                if group.phase != PairPhase::Preparing || group.carrier.is_some() {
                    return Err(JournalError::Conflict);
                }
                let inner = record.attempt(group.inner)?;
                if inner.nonce != attempt.nonce.checked_add(1).ok_or(JournalError::Identity)? {
                    return Err(JournalError::Identity);
                }
                attempt.id =
                    u64::try_from(record.attempts.len()).map_err(|_| JournalError::Identity)?;
                attempt.material = inner.material.clone();
                let group = &mut record.shield_pairs[index];
                group.carrier = Some(attempt.id);
                group.phase = PairPhase::Prepared;
                group.extensions.insert("carrier_context".into(), evidence);
                group
                    .extensions
                    .insert("sealing_key_hash".into(), serde_json::json!(key_hash));
                record.attempts.push(attempt);
                Ok(())
            })
        })
        .await
        .map_err(|_| JournalError::Storage("shield publication worker stopped".into()))?
    }
    pub(crate) async fn authorize(
        self,
        client: &Client,
        pair: u64,
        inner: &UnsignedExtrinsic,
        carrier: &UnsignedExtrinsic,
        context: &QualifiedShieldRuntime,
    ) -> Result<(rtao_rpc::AuthorOwnership, rtao_rpc::AuthorRetirement), JournalError> {
        // Recheck application semantics on the actual inner, never ciphertext.
        self.check_inner(client, inner, context).await?;
        context.check(&self.policy, carrier)?;
        if inner.address() != carrier.address()
            || inner.genesis_hash() != carrier.genesis_hash()
            || inner.crypto_type() != carrier.crypto_type()
        {
            return Err(JournalError::Identity);
        }
        let inner_identity = (
            inner.nonce(),
            inner.era_block_hash(),
            digest(inner.payload()),
        );
        let carrier_identity = (
            carrier.nonce(),
            carrier.era_block_hash(),
            digest(carrier.payload()),
        );
        let code = serde_json::json!(context.code_hash);
        let claims = self.claims.clone();
        let binding = self.binding.clone();
        let policy = self.policy.clone();
        let hash = crate::offload(move || {
            let _claims = claims;
            binding.journal.update(|image| {
                let revision = image
                    .revision
                    .checked_add(1)
                    .ok_or(JournalError::Identity)?;
                let record = image
                    .operations
                    .get_mut(&binding.id)
                    .ok_or(JournalError::Identity)?;
                admission(record, &binding, &policy, &image.control)?;
                let index = usize::try_from(pair).map_err(|_| JournalError::Identity)?;
                let group = record
                    .shield_pairs
                    .get(index)
                    .ok_or(JournalError::Identity)?;
                if group.phase != PairPhase::Prepared {
                    return Err(JournalError::Conflict);
                }
                let inner = record.attempt(group.inner)?;
                let carrier = record.attempt(group.carrier.ok_or(JournalError::Identity)?)?;
                if (inner.nonce, inner.era_anchor) != (inner_identity.0, inner_identity.1)
                    || (carrier.nonce, carrier.era_anchor)
                        != (carrier_identity.0, carrier_identity.1)
                    || inner.extensions.get("signing_payload_hash")
                        != Some(&serde_json::json!(inner_identity.2))
                    || carrier.extensions.get("signing_payload_hash")
                        != Some(&serde_json::json!(carrier_identity.2))
                    || group
                        .extensions
                        .get("signing_context")
                        .and_then(|v| v.get("runtime_code_hash"))
                        != Some(&code)
                    || group
                        .extensions
                        .get("carrier_context")
                        .and_then(|v| v.get("runtime_code_hash"))
                        != Some(&code)
                {
                    return Err(JournalError::Blocked(
                        "shield signed pair context changed before authorization".into(),
                    ));
                }
                let hash = carrier.hash;
                record.shield_pairs[index].phase = PairPhase::DispatchAuthorized { revision };
                record.incompatibility = None;
                Ok(hash)
            })
        })
        .await
        .map_err(|_| JournalError::Storage("shield authorization worker stopped".into()))??;
        Ok(rtao_rpc::AuthorOwnership::new(
            rtao_rpc::H256(hash.0),
            self.claims,
        ))
    }
}
