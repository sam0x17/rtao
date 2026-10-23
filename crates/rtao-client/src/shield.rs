// Derived from RaoFoundation/subtensor sdk/python/bittensor/executor.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Paired-nonce MEV shield submission and bounded inner-receipt resolution.

use crate::{
    BlockRef, CallBytes, Client, ClientError, ExtrinsicReport, Mortality, Period, SubmitFailure,
    SubmitOptions, ViewExt, Wait,
    submit::{
        OperationLimit, SubmissionAttempt, SubmissionLimits, submission_attempt_error, unknown_fate,
    },
};
use rtao_core::CoreError;
use rtao_primitives::{AccountId, Balance, CryptoType, H256, Tao};
use rtao_signer::{SignPurpose, Signer};
use serde_json::json;
use std::time::Duration;

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
            submit: SubmitOptions::default(),
        }
    }
}

/// Default re-seals after a carrier whose decrypted inner was proven never included.
pub const SHIELD_DROPPED_INNER_RETRIES: u32 = 3;

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

struct ShieldReceiptWindow {
    finalized_anchor: u64,
    expiry: u64,
    period: ShieldPeriod,
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
    /// fate. When the carrier was included but the decrypted inner is proven
    /// absent through its finalized era expiry, the call never executed, so it
    /// is re-sealed with a fresh key and nonce pair, up to
    /// [`ShieldOptions::dropped_inner_retries`] more times. On fast development chains
    /// the per-block key rotation leaves a carrier well under a second to land.
    pub async fn submit_shielded(
        &self,
        call: CallBytes,
        signer: &dyn Signer,
        opts: &ShieldOptions,
    ) -> Result<ExtrinsicReport, SubmitFailure> {
        if opts.submit.journal.is_some() {
            return Err(ClientError::Journal(crate::journal::JournalError::Blocked(
                "journaled paired shield recovery requires a dedicated adapter".into(),
            ))
            .into());
        }
        let mut retries = opts.dropped_inner_retries;
        loop {
            let report = self
                .submit_shielded_once(call.clone(), signer, opts)
                .await?;
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

    async fn submit_shielded_once(
        &self,
        call: CallBytes,
        signer: &dyn Signer,
        opts: &ShieldOptions,
    ) -> Result<ExtrinsicReport, SubmitFailure> {
        let mut submit = opts.submit.clone();
        submit.mortality = Mortality::Mortal(Period::new(opts.period.get()));
        // Resolve the carrier on inclusion; finality is verified for the inner
        // against the canonical chain, including a reorg during the wait.
        submit.wait = if opts.wait == Wait::Pool {
            Wait::Pool
        } else {
            Wait::Inclusion
        };
        let limits = SubmissionLimits::new(&submit)?;
        let attempted_hash = SubmissionAttempt::default();
        let operation = async {
            limits.admission.run(self, self.ensure_signing()).await?;
            let account = signer.account();
            let crypto_type = signer.crypto_type();
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
                    Ok(()) if roomy || waiting.elapsed() >= opts.finality_wait => break finalized,
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
            let (carrier_lease, inner_lease) = limits
                .admission
                .run(self, self.nonces().lease_pair(&account))
                .await?;
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
            let mut carrier_report = match self
                .send_and_watch(&bytes, hash, &submit, limits.admission, &attempted_hash)
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
        limits.full.run(self, operation).await.map_err(|error| {
            submission_attempt_error(self, signer.account(), error, &attempted_hash)
        })
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
