// Derived from RaoFoundation/subtensor sdk/python/bittensor/executor.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Paired-nonce MEV shield submission and bounded inner-receipt resolution.

use crate::{
    BlockRef, CallBytes, Client, ClientError, ExtrinsicReport, Mortality, Period, PoolRejection,
    SubmitFailure, SubmitOptions, ViewExt, Wait,
    submit::{
        OperationLimit, SubmissionAttempt, SubmissionLimits, submission_attempt_error, unknown_fate,
    },
};
use rtao_core::CoreError;
use rtao_primitives::{AccountId, Balance, CryptoType, H256, Tao};
use rtao_signer::{SignPurpose, Signer};
use serde_json::json;
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

/// The only mortal periods accepted by the runtime's shield mortality check.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ShieldPeriod {
    /// Four blocks; permits at most one block of finality lag.
    P4,
    /// Eight blocks; permits at most five blocks of finality lag.
    #[default]
    P8,
}
impl ShieldPeriod {
    /// Exact era period in blocks.
    pub const fn get(self) -> u64 {
        match self {
            Self::P4 => 4,
            Self::P8 => 8,
        }
    }
}
impl TryFrom<u64> for ShieldPeriod {
    type Error = ClientError;
    fn try_from(period: u64) -> Result<Self, Self::Error> {
        match period {
            4 => Ok(Self::P4),
            8 => Ok(Self::P8),
            _ => Err(ClientError::Protocol(
                "shield period must be exactly 4 or 8 blocks".into(),
            )),
        }
    }
}

/// Runtime-bounded encrypted carrier payload, validated before carrier signing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShieldCiphertext(Vec<u8>);
impl ShieldCiphertext {
    /// Reject a payload above the extracted Python/runtime ciphertext bound.
    pub fn new(bytes: Vec<u8>) -> Result<Self, ClientError> {
        let length = u64::try_from(bytes.len())
            .map_err(|_| ClientError::Protocol("shield ciphertext length overflows u64".into()))?;
        if length > rtao_catalog::py::MAX_SHIELDED_CIPHERTEXT_BYTES {
            return Err(ClientError::Protocol(format!(
                "shield ciphertext has {length} bytes; maximum is {}",
                rtao_catalog::py::MAX_SHIELDED_CIPHERTEXT_BYTES
            )));
        }
        Ok(Self(bytes))
    }
    /// Encrypt signed inner bytes with the core's key-hash-prefixed envelope.
    pub async fn seal(next_key: &[u8], inner: &[u8]) -> Result<Self, ClientError> {
        let next_key = next_key.to_vec();
        let inner = inner.to_vec();
        let sealed = rtao_core::offload(move || rtao_core::mlkem::seal(&next_key, &inner, true))
            .await
            .map_err(|error| {
                ClientError::Protocol(format!("shield encryption task failed: {error}"))
            })?
            .map_err(CoreError::from)?;
        Self::new(sealed)
    }
    /// Borrow the validated ciphertext.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
    /// Consume the validated ciphertext for metadata-directed composition.
    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }
}

/// Exact identities of the transmitted carrier and encrypted inner extrinsic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShieldReport {
    /// Identity of the only extrinsic transmitted to the author RPC.
    pub carrier_hash: H256,
    /// Identity scanned for after carrier inclusion.
    pub inner_hash: H256,
}

/// Shield preparation, admission and receipt-wait options.
#[derive(Clone, Debug)]
pub struct ShieldOptions {
    /// Restricted era shared by both signed extrinsics.
    pub period: ShieldPeriod,
    /// Return after carrier pool acceptance or resolved inner inclusion/finality.
    pub wait: Wait,
    /// Optional maximum inner-call fee, enforced before either signature.
    pub max_fee: Option<Balance<Tao>>,
    /// How long to wait for finality to leave room in the era before anchoring.
    ///
    /// The era is anchored at the finalized head. Fast development chains
    /// finalize in bursts, so a lag that would leave too little of the era for
    /// inclusion usually clears within a few seconds; a stalled or distant
    /// finality still refuses.
    pub finality_wait: Duration,
    /// Re-seals after a carrier whose decrypted inner was proven never included.
    ///
    /// The inner is proven absent through its finalized era expiry, so it never
    /// executed; each retry costs one more carrier fee.
    pub dropped_inner_retries: u32,
    /// Re-seals after a carrier lost its nonce to another transaction.
    ///
    /// A retry needs proof that the carrier was never included: the author
    /// refused it as stale or outbid, or the pool marked it invalid or usurped
    /// while the account nonce moved past it and finalized history then passed
    /// its era without it. Each retry waits one block, doubling to eight, and
    /// re-seals with a fresh key, anchor and nonce pair within this
    /// invocation's deadlines. A journaled submission counts these re-seals
    /// in its persisted pair budget and re-seals only after a finalized
    /// absence certificate past the era, reconciling again after a block
    /// backoff at most this many times while finality catches up.
    pub nonce_conflict_retries: u32,
    /// Shared deadlines, reconciliation and metadata-signing display settings.
    ///
    /// Its mortality, nonce and wait are replaced by the shield's paired nonces,
    /// restricted period and `wait` above.
    pub submit: SubmitOptions,
}
impl Default for ShieldOptions {
    fn default() -> Self {
        Self {
            period: ShieldPeriod::P8,
            wait: Wait::Inclusion,
            max_fee: None,
            finality_wait: SHIELD_FINALITY_WAIT,
            dropped_inner_retries: SHIELD_DROPPED_INNER_RETRIES,
            nonce_conflict_retries: SHIELD_NONCE_CONFLICT_RETRIES,
            submit: SubmitOptions::default(),
        }
    }
}

/// Default re-seals after a carrier whose decrypted inner was proven never included.
pub const SHIELD_DROPPED_INNER_RETRIES: u32 = 3;
/// Default re-seals after a carrier proven to have lost its nonce, never included.
pub const SHIELD_NONCE_CONFLICT_RETRIES: u32 = 10;

/// How a carrier failure may show that its nonce was taken.
#[derive(Clone, Copy)]
enum NonceConflict {
    /// The author refused the bytes, so they never entered any pool.
    Refused,
    /// The pool accepted and then discarded the carrier, which may have been
    /// gossiped; absence must be proven before re-sealing.
    Pooled,
}

fn nonce_conflict(error: &SubmitFailure, refused: bool) -> Option<NonceConflict> {
    let SubmitFailure::Pool(rejection) = error else {
        return None;
    };
    match rejection {
        PoolRejection::Stale | PoolRejection::PriorityTooLow if refused => {
            Some(NonceConflict::Refused)
        }
        PoolRejection::Watch(_)
            if !refused && matches!(rejection.name(), Some("Invalid" | "Usurped")) =>
        {
            Some(NonceConflict::Pooled)
        }
        _ => None,
    }
}

const DROPPED_INNER: &str =
    "the MEV shield accepted the encrypted submission, but the decrypted extrinsic (";

/// Whether a shielded report is an included carrier whose inner is proven
/// absent through finalized era expiry, so the call never executed.
fn inner_proven_dropped(report: &ExtrinsicReport) -> bool {
    !report.success
        && report.error.is_none()
        && report
            .shield
            .as_ref()
            .is_some_and(|shield| shield.carrier_hash == report.hash)
        && report.message.starts_with(DROPPED_INNER)
}

/// Default wait for finality to come within a shield era before refusing.
pub const SHIELD_FINALITY_WAIT: Duration = Duration::from_secs(60);
/// Interval between finality-lag checks while waiting for an era anchor.
const SHIELD_FINALITY_POLL: Duration = Duration::from_millis(100);

enum CarrierConflict {
    /// Proven never included; a fresh pair may be sealed.
    Reseal,
    /// Found in finalized history after all.
    Included(Box<ExtrinsicReport>),
    /// No proof; the original failure stands.
    Unproven,
}

struct ShieldReceiptWindow {
    finalized_anchor: u64,
    expiry: u64,
    period: ShieldPeriod,
}

/// Reconciled logical readiness before resolving a private shield signer.
/// Neither variant is a reusable author-I/O permit.
pub enum JournalShieldReadiness {
    /// Fresh preparation is permitted while this account/operation owner lives.
    Ready(crate::journal::JournalPreparation),
    /// An exact historical finalized inner effect already resolves the operation.
    Finalized(Box<ExtrinsicReport>),
}

fn recovered_call_result(
    binding: &crate::journal::JournalBinding,
    call: &CallBytes,
    report: Box<ExtrinsicReport>,
) -> Result<ExtrinsicReport, SubmitFailure> {
    if let Err(cause) = binding.check_recovered_call(&report, call) {
        return Err(SubmitFailure::RecoveryContext {
            receipt: report,
            cause,
        });
    }
    Ok(*report)
}

impl Client {
    /// Price the maximum-size carrier with public identity and a fresh nonce.
    pub async fn estimate_shielded_carrier_fee(
        &self,
        payer: AccountId,
    ) -> Result<Balance<Tao>, ClientError> {
        let runtime = self.refresh_runtime().await?;
        let length = usize::try_from(rtao_catalog::py::MAX_SHIELDED_CIPHERTEXT_BYTES)
            .map_err(|_| ClientError::Protocol("shield ciphertext bound exceeds usize".into()))?;
        let call = rtao_runtime::calls::mev_shield::submit_encrypted(vec![0; length])
            .compose(&runtime)
            .map_err(ClientError::from)?;
        let fee = self
            .estimate_weight(call, payer, CryptoType::Sr25519)
            .await?;
        Ok(Balance::from_rao(fee.partial_fee))
    }

    /// Sign and seal an inner call, then transmit only its signed carrier.
    ///
    /// The carrier uses nonce n and the inner n+1. Both share one finalized era
    /// anchor. Shielded carriers are never retried after rejection or unknown
    /// fate, except after proven non-inclusion of a carrier that lost its
    /// nonce to another transaction, up to
    /// [`ShieldOptions::nonce_conflict_retries`] times. When the carrier was
    /// included but the decrypted inner is proven absent through its finalized
    /// era expiry, the call never executed, so it is re-sealed with a fresh key
    /// and nonce pair, up to [`ShieldOptions::dropped_inner_retries`] more
    /// times. On fast development chains the per-block key rotation leaves a
    /// carrier well under a second to land.
    pub async fn submit_shielded(
        &self,
        call: CallBytes,
        signer: &dyn Signer,
        opts: &ShieldOptions,
    ) -> Result<ExtrinsicReport, SubmitFailure> {
        if opts.submit.journal.is_some() {
            let limits = SubmissionLimits::new(&opts.submit)?;
            match self
                .prepare_journal_shield_with_limits(opts, limits)
                .await?
            {
                JournalShieldReadiness::Finalized(report) => {
                    let verify = || {
                        let binding = opts
                            .submit
                            .journal
                            .as_ref()
                            .ok_or(crate::journal::JournalError::Identity)?;
                        if binding.record()?.intent.account != signer.account() {
                            return Err(crate::journal::JournalError::RecoveryContext(
                                crate::journal::RecoveryContextError::AccountMismatch,
                            ));
                        }
                        binding.check_recovered_call(&report, &call)
                    };
                    if let Err(cause) = verify() {
                        return Err(SubmitFailure::RecoveryContext {
                            receipt: report,
                            cause,
                        });
                    }
                    return Ok(*report);
                }
                JournalShieldReadiness::Ready(owner) => {
                    return Box::pin(
                        self.submit_journal_shield_owned(call, signer, opts, owner, limits),
                    )
                    .await;
                }
            }
        }
        let mut retries = opts.dropped_inner_retries;
        let mut conflicts = 0;
        loop {
            let limits = SubmissionLimits::new(&opts.submit)?;
            let report = loop {
                let proven = AtomicBool::new(false);
                let watch = (conflicts < opts.nonce_conflict_retries).then_some(&proven);
                // Keep the signing/recovery state machine out of every caller's
                // future frame. This remains one owned, cancellable operation.
                match Box::pin(self.submit_shielded_attempt(
                    call.clone(),
                    signer,
                    opts,
                    limits,
                    watch,
                ))
                .await
                {
                    Ok(report) => break report,
                    Err(error) if proven.load(Ordering::Acquire) => {
                        let delay =
                            crate::nonce::block_backoff(self.block_time().await?, conflicts);
                        conflicts += 1;
                        tracing::warn!(
                            %error,
                            attempt = conflicts,
                            ?delay,
                            "shield carrier lost its nonce to another transaction and was \
                             proven never included; re-sealing with a fresh nonce pair"
                        );
                        limits
                            .full
                            .run(self, async {
                                tokio::time::sleep(delay).await;
                                Ok::<(), ClientError>(())
                            })
                            .await?;
                    }
                    Err(error) => return Err(error),
                }
            };
            if retries == 0 || !inner_proven_dropped(&report) {
                return Ok(report);
            }
            retries -= 1;
            tracing::warn!(
                carrier = %crate::hash_hex(report.hash),
                "shielded inner extrinsic was dropped before its era expired; re-sealing"
            );
        }
    }

    /// Reconcile a journaled shield operation before a caller unlocks its signer.
    /// Holds actual account/operation ownership for preparation, or returns the
    /// exact historical effect.
    pub async fn prepare_journal_shield(
        &self,
        opts: &ShieldOptions,
    ) -> Result<JournalShieldReadiness, SubmitFailure> {
        Box::pin(
            self.prepare_journal_shield_with_limits(opts, SubmissionLimits::new(&opts.submit)?),
        )
        .await
    }
    async fn prepare_journal_shield_with_limits(
        &self,
        opts: &ShieldOptions,
        limits: SubmissionLimits,
    ) -> Result<JournalShieldReadiness, SubmitFailure> {
        if opts.wait != Wait::Finalization {
            return Err(ClientError::Journal(crate::journal::JournalError::Blocked(
                "durable shield requires finalized inner wait".into(),
            ))
            .into());
        }
        let binding = opts
            .submit
            .journal
            .as_ref()
            .ok_or_else(|| ClientError::Journal(crate::journal::JournalError::Identity))?;
        let owner = binding.reserve().map_err(ClientError::Journal)?;
        let scoped = owner.binding();
        let record = scoped.record().map_err(ClientError::Journal)?;
        if record.intent.genesis != self.genesis() {
            return Err(ClientError::Journal(crate::journal::JournalError::Identity).into());
        }
        limits.full.run(self, self.native_author_session()).await?;
        if let Some(pair) = record.shield_pairs.iter().rev().find(|p| {
            matches!(
                p.phase,
                crate::journal::PairPhase::DispatchAuthorized { .. }
            )
        }) {
            let inner = record
                .attempt(pair.inner)
                .map_err(ClientError::Journal)?
                .hash;
            let carrier = record
                .attempt(
                    pair.carrier
                        .ok_or(ClientError::Journal(crate::journal::JournalError::Identity))?,
                )
                .map_err(ClientError::Journal)?
                .hash;
            let pending = |reason| SubmitFailure::PendingShield {
                operation: binding.id().to_owned(),
                pair: pair.id,
                carrier_hash: carrier,
                inner_hash: inner,
                reason,
            };
            match limits
                .full
                .run(
                    self,
                    Box::pin(async {
                        scoped
                            .recover(self, 128)
                            .await
                            .map_err(ClientError::Journal)
                    }),
                )
                .await
            {
                Ok(crate::journal::RecoveryStatus::Finalized(report)) => {
                    return Ok(JournalShieldReadiness::Finalized(report));
                }
                Ok(crate::journal::RecoveryStatus::Rebuildable) => {}
                Ok(crate::journal::RecoveryStatus::Unresolved(reason)) => {
                    return Err(pending(reason));
                }
                Err(_) => {
                    return Err(pending(
                        crate::journal::RecoveryReason::FinalizedUnavailable,
                    ));
                }
            }
        }
        limits.admission.check(self)?;
        let policy = scoped
            .shield_policy(
                opts.period.get(),
                opts.dropped_inner_retries,
                opts.nonce_conflict_retries,
                opts.max_fee.map(|f| f.rao()),
            )
            .map_err(ClientError::Journal)?;
        let preparation = scoped.claim_shield(policy).map_err(ClientError::Journal)?;
        limits
            .admission
            .run(self, async {
                scoped
                    .qualify_shield_head(self)
                    .await
                    .map_err(ClientError::Journal)
            })
            .await?;
        limits.admission.run(self, self.ensure_signing()).await?;
        drop(preparation);
        Ok(JournalShieldReadiness::Ready(owner))
    }
    async fn submit_journal_shield_owned(
        &self,
        call: CallBytes,
        signer: &dyn Signer,
        opts: &ShieldOptions,
        owner: crate::journal::JournalPreparation,
        limits: SubmissionLimits,
    ) -> Result<ExtrinsicReport, SubmitFailure> {
        let mut scoped = opts.clone();
        scoped.submit.journal = Some(owner.binding());
        let mut last = None;
        let mut waits = 0;
        // This invocation cannot renew the persisted pair limit or either
        // original deadline, even when a typed negative result permits reseal.
        // A pair that is not yet provable, such as a carrier that lost its
        // nonce, is reconciled again after a block backoff, at most
        // `nonce_conflict_retries` times, before it is reported as pending.
        let pairs = opts
            .dropped_inner_retries
            .saturating_add(opts.nonce_conflict_retries);
        for _ in 0..=pairs {
            // Bound the parent future and debug-build polling stack across this
            // large signing/RPC state machine. This does not spawn or detach it.
            let result =
                Box::pin(self.submit_shielded_attempt(call.clone(), signer, &scoped, limits, None))
                    .await;
            let Err(error @ SubmitFailure::PendingShield { .. }) = result else {
                return result;
            };
            let SubmitFailure::PendingShield {
                operation,
                pair,
                carrier_hash,
                inner_hash,
                ..
            } = &error
            else {
                return Err(error);
            };
            let (operation, pair) = (operation.clone(), *pair);
            let (carrier_hash, inner_hash) = (*carrier_hash, *inner_hash);
            let pending = |reason| SubmitFailure::PendingShield {
                operation: operation.clone(),
                pair,
                carrier_hash,
                inner_hash,
                reason,
            };
            let binding = scoped
                .submit
                .journal
                .as_ref()
                .ok_or(ClientError::Journal(crate::journal::JournalError::Identity))?;
            loop {
                match limits
                    .full
                    .run(
                        self,
                        Box::pin(async {
                            binding
                                .recover(self, 128)
                                .await
                                .map_err(ClientError::Journal)
                        }),
                    )
                    .await
                {
                    Ok(crate::journal::RecoveryStatus::Finalized(report)) => {
                        return recovered_call_result(binding, &call, report);
                    }
                    Ok(crate::journal::RecoveryStatus::Rebuildable) => {
                        last = Some(error);
                        break;
                    }
                    Ok(crate::journal::RecoveryStatus::Unresolved(
                        reason @ crate::journal::RecoveryReason::AwaitingFinality { .. },
                    )) if waits < opts.nonce_conflict_retries => {
                        let delay = crate::nonce::block_backoff(self.block_time().await?, waits);
                        waits += 1;
                        tracing::info!(
                            operation = %operation,
                            pair,
                            attempt = waits,
                            ?delay,
                            "journaled shield pair awaits finalized expiry; reconciling again"
                        );
                        let slept = limits
                            .full
                            .run(self, async {
                                tokio::time::sleep(delay).await;
                                Ok::<(), ClientError>(())
                            })
                            .await;
                        if slept.is_err() {
                            return Err(pending(reason));
                        }
                    }
                    Ok(crate::journal::RecoveryStatus::Unresolved(reason)) => {
                        return Err(pending(reason));
                    }
                    Err(_) => {
                        return Err(pending(
                            crate::journal::RecoveryReason::FinalizedUnavailable,
                        ));
                    }
                }
            }
            tracing::warn!(
                operation = %operation,
                pair,
                "journaled shield pair proven never executed; a new pair may follow \
                 within the persisted pair budget"
            );
        }
        Err(last.unwrap_or_else(|| {
            ClientError::Journal(crate::journal::JournalError::Blocked(
                "shield invocation exhausted its original attempt budget".into(),
            ))
            .into()
        }))
    }

    #[cfg(test)]
    async fn submit_shielded_once(
        &self,
        call: CallBytes,
        signer: &dyn Signer,
        opts: &ShieldOptions,
    ) -> Result<ExtrinsicReport, SubmitFailure> {
        Box::pin(self.submit_shielded_attempt(
            call,
            signer,
            opts,
            SubmissionLimits::new(&opts.submit)?,
            None,
        ))
        .await
    }
    /// One sealed pair. With `conflict`, a carrier failure that is proven to
    /// have lost its nonce and never been included sets it before returning.
    async fn submit_shielded_attempt(
        &self,
        call: CallBytes,
        signer: &dyn Signer,
        opts: &ShieldOptions,
        limits: SubmissionLimits,
        conflict: Option<&AtomicBool>,
    ) -> Result<ExtrinsicReport, SubmitFailure> {
        // Durable submissions stay fail-closed with their own recovery.
        let conflict = conflict.filter(|_| opts.submit.journal.is_none());
        let mut submit = opts.submit.clone();
        submit.mortality = Mortality::Mortal(Period::new(opts.period.get()));
        // Resolve the carrier on inclusion; finality is verified for the inner
        // against the canonical chain, including a reorg during the wait.
        submit.wait = if opts.wait == Wait::Pool {
            Wait::Pool
        } else {
            Wait::Inclusion
        };
        let attempted_hash = SubmissionAttempt::default();
        let authorization = std::sync::OnceLock::<(u64, H256, H256)>::new();
        let operation = async {
            let account = signer.account();
            let crypto_type = signer.crypto_type();
            let mut durable: Option<(
                crate::journal::ShieldPreparation,
                crate::journal::QualifiedShieldRuntime,
            )> = if let Some(binding) = &opts.submit.journal {
                if opts.wait != Wait::Finalization {
                    return Err(ClientError::Journal(crate::journal::JournalError::Blocked(
                        "durable shield requires finalized inner wait".into(),
                    ))
                    .into());
                }
                if binding
                    .record()
                    .map_err(ClientError::Journal)?
                    .intent
                    .account
                    != account
                {
                    return Err(ClientError::Journal(crate::journal::JournalError::Identity).into());
                }
                limits
                    .admission
                    .run(self, self.native_author_session())
                    .await?;
                let policy = binding
                    .shield_policy(
                        opts.period.get(),
                        opts.dropped_inner_retries,
                        opts.nonce_conflict_retries,
                        opts.max_fee.map(|fee| fee.rao()),
                    )
                    .map_err(ClientError::Journal)?;
                let retained = binding.record().map_err(ClientError::Journal)?;
                if let Some(pair) = retained.shield_pairs.iter().rev().find(|p| {
                    matches!(
                        p.phase,
                        crate::journal::PairPhase::DispatchAuthorized { .. }
                    )
                }) {
                    let inner = retained
                        .attempt(pair.inner)
                        .map_err(ClientError::Journal)?
                        .hash;
                    let carrier =
                        retained
                            .attempt(pair.carrier.ok_or(ClientError::Journal(
                                crate::journal::JournalError::Identity,
                            ))?)
                            .map_err(ClientError::Journal)?
                            .hash;
                    match limits
                        .admission
                        .run(self, async {
                            binding
                                .recover(self, 128)
                                .await
                                .map_err(ClientError::Journal)
                        })
                        .await?
                    {
                        crate::journal::RecoveryStatus::Finalized(report) => {
                            return recovered_call_result(binding, &call, report);
                        }
                        crate::journal::RecoveryStatus::Rebuildable => {}
                        crate::journal::RecoveryStatus::Unresolved(reason) => {
                            return Err(SubmitFailure::PendingShield {
                                operation: binding.id().to_owned(),
                                pair: pair.id,
                                carrier_hash: carrier,
                                inner_hash: inner,
                                reason,
                            });
                        }
                    }
                }
                let preparation = binding.claim_shield(policy).map_err(ClientError::Journal)?;
                let context = limits
                    .admission
                    .run(self, async {
                        binding
                            .qualify_shield_head(self)
                            .await
                            .map_err(ClientError::Journal)
                    })
                    .await?;
                Some((preparation, context))
            } else {
                None
            };
            // Historical reconciliation above must work even when the current
            // runtime cannot sign. New preparation still checks signing status.
            limits.admission.run(self, self.ensure_signing()).await?;
            if let Some(max_fee) = opts.max_fee {
                let fee = limits
                    .admission
                    .run(
                        self,
                        self.estimate_weight(call.clone(), account, crypto_type),
                    )
                    .await?;
                if fee.partial_fee > max_fee.rao() {
                    return Err(ClientError::Protocol(format!(
                        "inner shield fee {} rao exceeds maximum {} rao",
                        fee.partial_fee,
                        max_fee.rao()
                    ))
                    .into());
                }
            }
            // NextKey rotates every block and the carrier is included where it
            // is submitted, so the key and the era guard's best height come
            // from the submission endpoint. A lagging read endpoint would
            // yield an expired key and an undecryptable, silently dropped
            // inner. Without a submission endpoint this is the client itself.
            let chain = limits.admission.run(self, self.submit_client()).await?;
            // Fail before signer preparation when the shield is inactive.
            limits.admission.run(self, chain.shield_next_key()).await?;
            limits
                .admission
                .run(self, async {
                    signer
                        .prepare_shielded()
                        .await
                        .map_err(ClientError::Signing)
                })
                .await?;
            if signer.account() != account || signer.crypto_type() != crypto_type {
                return Err(
                    ClientError::Signing(rtao_signer::SignerError::InvalidSignature(
                        "signer identity changed during shield preparation".into(),
                    ))
                    .into(),
                );
            }
            // Interactive preparation may span many blocks. Both heights used
            // by the era guard must be observed after the signer is ready.
            // Wait, within the admission deadline, until finality leaves at
            // least half the era for inclusion before anchoring it. After the
            // wait, proceed if the era can still cover inclusion, else refuse.
            // The anchor is the read endpoint's finalized head, where the
            // signing payload resolves it; best is the submission endpoint's.
            // A lagging read endpoint only widens the measured lag.
            //
            // The pair is leased only once an anchor passes, so a refusal
            // reserves nothing. A fresh lease may wait several blocks for this
            // account's pending carriers, which ages that anchor; after such a
            // wait the anchor is chosen again, holding the lease.
            let mut leased = None;
            let (finalized, (carrier_lease, inner_lease)) = loop {
                let waiting = tokio::time::Instant::now();
                let finalized = loop {
                    let best = limits
                        .admission
                        .run(self, chain.at(BlockRef::Best))
                        .await?
                        .block()
                        .number;
                    let finalized = limits
                        .admission
                        .run(self, self.at(BlockRef::Finalized))
                        .await?
                        .block()
                        .number;
                    let roomy = best.saturating_sub(finalized) <= era_headroom_lag(opts.period);
                    match check_era(opts.period, best, finalized) {
                        Ok(()) if roomy || waiting.elapsed() >= opts.finality_wait => {
                            break finalized;
                        }
                        Err(refusal) if waiting.elapsed() >= opts.finality_wait => {
                            return Err(refusal);
                        }
                        _ => {
                            limits
                                .admission
                                .run(self, async {
                                    tokio::time::sleep(SHIELD_FINALITY_POLL).await;
                                    Ok::<(), ClientError>(())
                                })
                                .await?;
                        }
                    }
                };
                if let Some(pair) = leased.take() {
                    break (finalized, pair);
                }
                if durable.is_some() {
                    // Other clients/restarts sharing the journal do not share
                    // this connection's nonce cache. The account owner is held
                    // here; resolve the actual author endpoint before
                    // allocating a pair.
                    self.nonces().clear_account(&account);
                }
                let (pair, waited) = limits
                    .admission
                    .run(self, self.nonces().lease_pair_waited(&account))
                    .await?;
                if !waited {
                    break (finalized, pair);
                }
                leased = Some(pair);
            };
            let inner = limits
                .admission
                .run(
                    self,
                    self.prepare_at_nonce_anchored(
                        call,
                        signer,
                        &submit,
                        inner_lease.value(),
                        Some(finalized),
                    ),
                )
                .await?;
            let receipt_window = ShieldReceiptWindow {
                finalized_anchor: finalized,
                expiry: inner.expiry().ok_or_else(|| {
                    ClientError::Protocol("shielded inner extrinsic has no mortal expiry".into())
                })?,
                period: opts.period,
            };
            if let Some((preparation, context)) = &mut durable {
                let binding =
                    opts.submit.journal.as_ref().ok_or_else(|| {
                        ClientError::Journal(crate::journal::JournalError::Identity)
                    })?;
                *context = limits
                    .admission
                    .run(self, async {
                        binding
                            .qualify_shield_head(self)
                            .await
                            .map_err(ClientError::Journal)
                    })
                    .await?;
                limits
                    .admission
                    .run(self, async {
                        preparation
                            .check_inner(&chain, &inner, context)
                            .await
                            .map_err(ClientError::Journal)
                    })
                    .await?;
            }
            let signature = limits
                .admission
                .run(self, async {
                    signer
                        .sign(&inner.signing_request(SignPurpose::ShieldInner))
                        .await
                        .map_err(ClientError::Signing)
                })
                .await?;
            let (inner_bytes, inner_hash) = limits
                .admission
                .run(self, self.attach_signature(&inner, signature))
                .await?;
            let pair = if let Some((preparation, context)) = &durable {
                Some(
                    limits
                        .admission
                        .run(self, async {
                            preparation
                                .publish_inner(&inner, &inner_bytes, inner_hash, context)
                                .await
                                .map_err(ClientError::Journal)
                        })
                        .await?,
                )
            } else {
                None
            };
            // Seal with the key current now: signer preparation and the
            // finality wait can outlast a key rotation on fast chains, and a
            // stale key leaves the inner undecryptable and silently dropped.
            let next_key = limits.admission.run(self, chain.shield_next_key()).await?;
            let ciphertext = limits
                .admission
                .run(self, ShieldCiphertext::seal(&next_key, &inner_bytes))
                .await?;
            let runtime = limits.admission.run(self, self.refresh_runtime()).await?;
            inner
                .call()
                .ensure_runtime(&runtime)
                .map_err(ClientError::from)?;
            let carrier =
                rtao_runtime::calls::mev_shield::submit_encrypted(ciphertext.into_bytes())
                    .compose(&runtime)
                    .map_err(ClientError::from)?;
            let unsigned = limits
                .admission
                .run(
                    self,
                    self.prepare_at_nonce_anchored(
                        carrier,
                        signer,
                        &submit,
                        carrier_lease.value(),
                        Some(finalized),
                    ),
                )
                .await?;
            let current_best = limits
                .admission
                .run(self, chain.at(BlockRef::Best))
                .await?
                .block()
                .number;
            check_era(opts.period, current_best, finalized)?;
            let runtime = limits.admission.run(self, self.refresh_runtime()).await?;
            inner
                .call()
                .ensure_runtime(&runtime)
                .map_err(ClientError::from)?;
            let signature = limits
                .admission
                .run(self, async {
                    signer
                        .sign(&unsigned.signing_request(SignPurpose::ShieldCarrier))
                        .await
                        .map_err(ClientError::Signing)
                })
                .await?;
            let (bytes, hash) = limits
                .admission
                .run(self, self.attach_signature(&unsigned, signature))
                .await?;
            let current_best = limits
                .admission
                .run(self, chain.at(BlockRef::Best))
                .await?
                .block()
                .number;
            check_era(opts.period, current_best, finalized)?;
            let mut inner_lease = Some(inner_lease);
            let runtime = limits.admission.run(self, self.refresh_runtime()).await?;
            inner
                .call()
                .ensure_runtime(&runtime)
                .map_err(ClientError::from)?;
            let ownership = if let Some((preparation, context)) = durable.take() {
                let pair = pair
                    .ok_or_else(|| ClientError::Journal(crate::journal::JournalError::Identity))?;
                limits
                    .admission
                    .run(self, async {
                        preparation
                            .publish_carrier(pair, &unsigned, &bytes, hash, &next_key, &context)
                            .await
                            .map_err(ClientError::Journal)
                    })
                    .await?;
                let binding =
                    opts.submit.journal.as_ref().ok_or_else(|| {
                        ClientError::Journal(crate::journal::JournalError::Identity)
                    })?;
                let fresh = limits
                    .admission
                    .run(self, async {
                        binding
                            .qualify_shield_head(self)
                            .await
                            .map_err(ClientError::Journal)
                    })
                    .await?;
                // Retain identities outside the deadline-cancelled future. A
                // publication error around authorization may still be durable.
                let _ = authorization.set((pair, hash, inner_hash));
                let (owner, _retirement) = limits
                    .admission
                    .run(self, async {
                        preparation
                            .authorize(&chain, pair, &inner, &unsigned, &fresh)
                            .await
                            .map_err(ClientError::Journal)
                    })
                    .await?;
                Some(owner)
            } else {
                None
            };
            let mut carrier_report = match self
                .send_and_watch(
                    &bytes,
                    hash,
                    &submit,
                    limits.admission,
                    &attempted_hash,
                    ownership,
                )
                .await
            {
                Ok(report) => {
                    limits.full.check(self)?;
                    carrier_lease.settle();
                    report
                }
                Err(error) => {
                    drop(carrier_lease);
                    drop(inner_lease.take());
                    let error =
                        submission_attempt_error(self, signer.account(), error, &attempted_hash);
                    if (submit.reconcile && unknown_fate(&error)) || attempted_hash.was_retracted()
                    {
                        limits.full.check(self)?;
                        self.reconcile(hash, unsigned.submission_head(), unsigned.expiry())
                            .await?
                    } else if let Some(proven) = conflict
                        && let Some(kind) = nonce_conflict(&error, attempted_hash.was_refused())
                    {
                        limits.full.check(self)?;
                        match self
                            .prove_carrier_conflict(kind, account, hash, &unsigned)
                            .await?
                        {
                            CarrierConflict::Reseal => {
                                proven.store(true, Ordering::Release);
                                return Err(error);
                            }
                            CarrierConflict::Included(report) => *report,
                            CarrierConflict::Unproven => return Err(error),
                        }
                    } else {
                        return Err(error);
                    }
                }
            };
            carrier_report.shield = Some(ShieldReport {
                carrier_hash: hash,
                inner_hash,
            });
            if opts.wait == Wait::Pool && carrier_report.block_number.is_none() {
                limits.full.check(self)?;
                if let Some(lease) = inner_lease.take() {
                    lease.settle();
                }
                return Ok(carrier_report);
            }
            if !carrier_report.success {
                if opts.wait == Wait::Finalization {
                    let block = carrier_report.block_number.ok_or_else(|| {
                        ClientError::Protocol(
                            "failed shield carrier has no inclusion height".into(),
                        )
                    })?;
                    let interval = self.block_time().await?;
                    let max_polls = rtao_catalog::py::SHIELDED_POLL_ATTEMPTS_PER_BLOCK
                        .saturating_mul(opts.period.get().saturating_add(1));
                    // The carrier's receipt is read where it was submitted.
                    let canonical = chain
                        .shield_finalized_hash(block, interval, max_polls, limits.full)
                        .await?;
                    if chain
                        .find_extrinsic_complete(hash, canonical)
                        .await?
                        .is_none()
                    {
                        carrier_report = self
                            .reconcile(hash, unsigned.submission_head(), unsigned.expiry())
                            .await?;
                    } else {
                        carrier_report = self.resolve_extrinsic(hash, canonical, true).await?;
                    }
                    carrier_report.shield = Some(ShieldReport {
                        carrier_hash: hash,
                        inner_hash,
                    });
                }
                if !carrier_report.success {
                    return Ok(carrier_report);
                }
            }
            let report = self
                .resolve_shielded_inner(
                    carrier_report,
                    inner_hash,
                    receipt_window,
                    opts.wait,
                    limits.full,
                )
                .await?;
            limits.full.check(self)?;
            if report.hash == inner_hash
                && let Some(lease) = inner_lease.take()
            {
                lease.settle();
            }
            Ok(report)
        };
        let outcome = limits.full.run(self, operation).await.map_err(|error| {
            submission_attempt_error(self, signer.account(), error, &attempted_hash)
        });
        let Some(binding) = &opts.submit.journal else {
            return outcome;
        };
        let Some(&(pair, carrier_hash, inner_hash)) = authorization.get() else {
            return outcome;
        };
        let pending = |reason| SubmitFailure::PendingShield {
            operation: binding.id().to_owned(),
            pair,
            carrier_hash,
            inner_hash,
            reason,
        };
        match outcome {
            Ok(report)
                if report.hash == inner_hash && report.inclusion == crate::Inclusion::Finalized =>
            {
                if binding.observed(&report).await.is_err() {
                    return Err(pending(crate::journal::RecoveryReason::ReceiptUnavailable));
                }
                match binding
                    .record()
                    .and_then(|r| r.finalized_effect().map(|effect| effect.map(|a| a.hash)))
                {
                    Ok(Some(hash)) if hash == inner_hash => Ok(report),
                    _ => Err(pending(crate::journal::RecoveryReason::EvidenceConflict)),
                }
            }
            // Even Closed/Policy/pool rejection or a finalized failed CARRIER
            // leaves the authorized inner explicitly unresolved, never failed.
            _ => Err(pending(crate::journal::RecoveryReason::InnerUnresolved)),
        }
    }

    /// Decide whether a failed carrier lost its nonce and was never included.
    ///
    /// An author refusal never entered a pool. A carrier the pool accepted and
    /// then discarded counts only when the account nonce has moved past it,
    /// so another transaction holds that nonce, and finalized history then
    /// passes its era without it. Reconciliation can instead find it included,
    /// and its inner is then resolved like any included carrier.
    async fn prove_carrier_conflict(
        &self,
        kind: NonceConflict,
        account: AccountId,
        hash: H256,
        unsigned: &crate::UnsignedExtrinsic,
    ) -> Result<CarrierConflict, SubmitFailure> {
        if matches!(kind, NonceConflict::Refused) {
            return Ok(CarrierConflict::Reseal);
        }
        match self.nonces().fresh(&account).await {
            Ok(next) if next > unsigned.nonce() => {}
            Ok(_) => return Ok(CarrierConflict::Unproven),
            Err(error) => {
                self.ensure_open()?;
                tracing::warn!(
                    %error,
                    "cannot read the account nonce after a discarded shield carrier; not re-sealing"
                );
                return Ok(CarrierConflict::Unproven);
            }
        }
        match self
            .reconcile(hash, unsigned.submission_head(), unsigned.expiry())
            .await
        {
            Ok(report) => Ok(CarrierConflict::Included(Box::new(report))),
            Err(SubmitFailure::Expired { .. }) => Ok(CarrierConflict::Reseal),
            Err(error) => {
                tracing::warn!(
                    %error,
                    carrier = %crate::hash_hex(hash),
                    "cannot prove that the discarded shield carrier was never included; \
                     not re-sealing"
                );
                Ok(CarrierConflict::Unproven)
            }
        }
    }

    /// Read the freshest MevShield NextKey this endpoint has.
    ///
    /// The key rotates every block and the author only accepts a carrier
    /// sealed to the key at the parent of the block it builds, so it is read
    /// without a block hash. A node answers from its best state, as a read at
    /// its best header would; an endpoint whose headers trail its state would
    /// otherwise return a key that has already expired. The era guard reads
    /// its best height separately.
    async fn shield_next_key(&self) -> Result<Vec<u8>, ClientError> {
        for attempt in 0..=2 {
            if let Some(key) = self
                .storage_unpinned(&rtao_runtime::storage::mev_shield::NEXT_KEY, ())
                .await?
                && !key.is_empty()
            {
                return Ok(key);
            }
            if attempt < 2 {
                tokio::time::sleep(self.block_time().await?).await;
            }
        }
        Err(ClientError::Protocol(
            "MEV Shield NextKey not available; is the MevShield pallet active?".into(),
        ))
    }

    async fn shield_block_hash(&self, number: u64) -> Result<Option<H256>, ClientError> {
        let value: serde_json::Value = self.request("chain_getBlockHash", json!([number])).await?;
        if value.is_null() {
            Ok(None)
        } else {
            crate::parse_hash(&value).map(Some)
        }
    }

    async fn resolve_shielded_inner(
        &self,
        mut outer: ExtrinsicReport,
        inner_hash: H256,
        window: ShieldReceiptWindow,
        wait: Wait,
        limit: OperationLimit,
    ) -> Result<ExtrinsicReport, SubmitFailure> {
        let Some(included_at) = outer.block_number else {
            return Ok(outer);
        };
        let deadline = window.expiry;
        let interval = self.block_time().await?;
        let max_polls = rtao_catalog::py::SHIELDED_POLL_ATTEMPTS_PER_BLOCK
            .checked_mul(window.period.get().saturating_add(1))
            .ok_or_else(|| ClientError::Protocol("shield poll bound overflow".into()))?;
        // The inner's receipt is scanned on the endpoint that received the
        // carrier, which sees best-chain blocks before any read endpoint.
        let chain = self.submit_client().await?;
        let mut block = included_at;
        // Python shares the missing-height allowance across the era range.
        // Four misses at one not-yet-produced height must not expire the scan.
        let mut waits_left = max_polls;
        let mut reorgs_left = rtao_catalog::py::SHIELDED_FINALITY_RPC_RETRIES;
        while block <= deadline {
            limit.check(self)?;
            let Some(block_hash) = chain.shield_block_hash(block).await? else {
                waits_left -= 1;
                if waits_left == 0 {
                    return Err(ClientError::Protocol(format!(
                        "shielded inner receipt remains unresolved: block {block} is unavailable after {max_polls} polls; canonical era expiry and absence have not been established"
                    ))
                    .into());
                }
                tokio::time::sleep(interval).await;
                continue;
            };
            if chain
                .find_extrinsic_complete(inner_hash, block_hash)
                .await?
                .is_none()
            {
                if block == deadline {
                    break;
                }
                block += 1;
                continue;
            }
            let resolved_hash = if wait == Wait::Finalization {
                chain
                    .shield_finalized_hash(block, interval, max_polls, limit)
                    .await?
            } else {
                block_hash
            };
            if resolved_hash != block_hash {
                let mut found = None;
                for attempt in 0..rtao_catalog::py::SHIELDED_FINALITY_RPC_RETRIES {
                    limit.check(self)?;
                    match tokio::time::timeout(
                        interval.max(Duration::from_millis(100)),
                        chain.find_extrinsic_complete(inner_hash, resolved_hash),
                    )
                    .await
                    {
                        Ok(Ok(receipt)) => {
                            found = Some(receipt);
                            break;
                        }
                        result => {
                            if attempt + 1 == rtao_catalog::py::SHIELDED_FINALITY_RPC_RETRIES {
                                return Err(ClientError::Protocol(format!("could not verify shielded inner extrinsic finalization after {} canonical receipt RPC attempts ({result:?})", rtao_catalog::py::SHIELDED_FINALITY_RPC_RETRIES)).into());
                            }
                            tokio::time::sleep(interval).await;
                        }
                    }
                }
                if found.flatten().is_none() {
                    // The finalized branch can include the inner at an earlier
                    // scanned height than the orphan. Revisit the finite era
                    // range after a reorg instead of only advancing.
                    if reorgs_left == 0 {
                        return Err(ClientError::Protocol(
                            "shield receipt chain kept reorganizing after finality".into(),
                        )
                        .into());
                    }
                    reorgs_left -= 1;
                    block = included_at;
                    continue;
                }
            }
            let mut inner = chain
                .resolve_extrinsic(inner_hash, resolved_hash, wait == Wait::Finalization)
                .await?;
            inner.shield = outer.shield;
            return Ok(inner);
        }
        // Seeing every best-chain height does not prove absence: an earlier
        // lookup may have inspected an orphan. First establish finalized
        // expiry, then rescan its canonical ancestors from the signing anchor.
        // The anchor also covers a carrier re-included earlier after a reorg.
        chain
            .shield_finalized_hash(deadline, interval, max_polls, limit)
            .await?;
        for height in window.finalized_anchor..=deadline {
            limit.check(self)?;
            let hash = chain.shield_block_hash(height).await?.ok_or_else(|| {
                ClientError::Protocol(format!(
                    "finalized shield receipt block {height} is unavailable; canonical absence has not been established"
                ))
            })?;
            if chain
                .find_extrinsic_complete(inner_hash, hash)
                .await?
                .is_some()
            {
                let mut inner = chain.resolve_extrinsic(inner_hash, hash, true).await?;
                inner.shield = outer.shield;
                return Ok(inner);
            }
        }
        outer.success = false;
        outer.message = format!(
            "{DROPPED_INNER}{}) was not included before its era expired",
            crate::hash_hex(inner_hash)
        );
        Ok(outer)
    }

    async fn shield_finalized_hash(
        &self,
        block: u64,
        interval: Duration,
        max_polls: u64,
        limit: OperationLimit,
    ) -> Result<H256, SubmitFailure> {
        let mut failures = 0;
        let mut finalized = false;
        for poll in 0..max_polls {
            limit.check(self)?;
            match tokio::time::timeout(
                interval.max(Duration::from_secs(1)),
                self.at(BlockRef::Finalized),
            )
            .await
            {
                Ok(Ok(head)) => {
                    failures = 0;
                    if head.block().number >= block {
                        finalized = true;
                        break;
                    }
                }
                result => {
                    failures += 1;
                    if failures >= rtao_catalog::py::SHIELDED_FINALITY_RPC_RETRIES {
                        return Err(ClientError::Protocol(format!("could not verify shielded inner extrinsic finalization after {failures} consecutive finalized-head RPC attempts ({result:?})")).into());
                    }
                }
            }
            if poll + 1 < max_polls {
                tokio::time::sleep(interval).await;
            }
        }
        if !finalized {
            return Err(ClientError::Protocol(format!(
                "shielded inner extrinsic at block {block} did not finalize after {max_polls} polls"
            ))
            .into());
        }
        for attempt in 0..rtao_catalog::py::SHIELDED_FINALITY_RPC_RETRIES {
            limit.check(self)?;
            if let Ok(Ok(Some(hash))) = tokio::time::timeout(
                interval.max(Duration::from_millis(100)),
                self.shield_block_hash(block),
            )
            .await
            {
                return Ok(hash);
            }
            if attempt + 1 < rtao_catalog::py::SHIELDED_FINALITY_RPC_RETRIES {
                tokio::time::sleep(interval).await;
            }
        }
        Err(ClientError::Protocol(format!("could not verify shielded inner extrinsic finalization after {} canonical block-hash RPC attempts", rtao_catalog::py::SHIELDED_FINALITY_RPC_RETRIES)).into())
    }
}

/// The finality lag that still leaves the carrier and its decrypted inner
/// extrinsic half the era to be included; waits aim for this, refusals use
/// `check_era`. Fast chains reach it right after each finality burst.
fn era_headroom_lag(period: ShieldPeriod) -> u64 {
    period.get() / 2
}

fn check_era(period: ShieldPeriod, best: u64, finalized: u64) -> Result<(), SubmitFailure> {
    let finality_lag = best.saturating_sub(finalized);
    if finality_lag >= period.get().saturating_sub(2) {
        Err(SubmitFailure::ShieldEraExpired {
            period: period.get(),
            finality_lag,
        })
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod durable_tests;
