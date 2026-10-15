// Derived from RaoFoundation/subtensor sdk/python/bittensor/_transport/interface.py, extrinsics.py and contract.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Finality-anchored transaction preparation and typed submission.

use crate::{BlockRef, Client, ClientError, NonceLease, View, ViewExt};
use bytes::Bytes;
use futures::StreamExt;
use rtao_core::{CoreError, codec::Value, digest::ChainInfo, extrinsic::TxParams};
use rtao_primitives::{AccountId, CryptoType, H256, TaoBalance};
use rtao_runtime::{CallBytes, RuntimeHandle};
use rtao_signer::{
    MultiSignature, SignPurpose, Signer, SignerCaps, SignerPayloadJson, SigningContext,
    SigningRequest,
};
use serde_json::json;
use std::{fmt, sync::OnceLock, time::Duration};

/// Normalized power-of-two era period in 4..=65536 blocks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Period(u64);
impl Period {
    /// Round and clamp without overflowing at large inputs.
    pub fn new(requested: u64) -> Self {
        Self(
            requested
                .checked_next_power_of_two()
                .unwrap_or(65536)
                .clamp(4, 65536),
        )
    }
    /// Recover the normalized block count.
    pub fn get(self) -> u64 {
        self.0
    }
}
/// Transaction validity era.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mortality {
    /// Indefinite validity anchored at genesis.
    Immortal,
    /// Bounded validity anchored at finalized history.
    Mortal(Period),
}
/// State at which a submission returns.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Wait {
    /// Return on pool acceptance before dispatch.
    Pool,
    /// Resolve an inclusion block without waiting for finality.
    Inclusion,
    /// Resolve a finalized inclusion block.
    #[default]
    Finalization,
}
/// Ordinary submission nonce selection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NoncePolicy {
    /// Acquire a shared nonce lease.
    #[default]
    Auto,
    /// Pin an explicit nonce in the shared manager.
    Explicit(u64),
}
/// Preparation and waiting options for one transaction.
#[derive(Clone, Debug)]
pub struct SubmitOptions {
    /// Validity era, normally the extracted 128-block default.
    pub mortality: Mortality,
    /// Inclusion state to await.
    pub wait: Wait,
    /// Cached allocation or explicit nonce.
    pub nonce: NoncePolicy,
    /// Deadline for the complete preparation, signing and watch operation.
    pub deadline: Option<Duration>,
    /// Deadline from entry through preparation, signing and author acknowledgment.
    ///
    /// This bound is released after pool or watch acknowledgment; inclusion and
    /// finality remain subject to `deadline`.
    pub submit_deadline: Option<Duration>,
    /// Scan finalized history after unknown fate or a dropped watch.
    pub reconcile: bool,
    /// Metadata-proof display decimals, matching the signer's configuration.
    pub metadata_decimals: u8,
    /// Metadata-proof token symbol, matching the signer's configuration.
    pub metadata_symbol: String,
}
impl Default for SubmitOptions {
    fn default() -> Self {
        Self {
            mortality: Mortality::Mortal(Period::new(rtao_catalog::py::DEFAULT_ERA_PERIOD)),
            wait: Wait::Finalization,
            nonce: NoncePolicy::Auto,
            deadline: None,
            submit_deadline: None,
            reconcile: false,
            metadata_decimals: 9,
            metadata_symbol: "TAO".into(),
        }
    }
}

/// Runtime-stamped prepared transaction and its immutable signer views.
///
/// Signing fields remain consistent with the transaction assembled later.
///
/// ```compile_fail
/// fn change_nonce(transaction: &mut rtao_client::UnsignedExtrinsic) {
///     transaction.nonce = 3;
/// }
/// ```
#[derive(Debug)]
pub struct UnsignedExtrinsic {
    /// Spec-stamped SCALE call bytes.
    pub(crate) call: CallBytes,
    /// Expected signing account.
    pub(crate) address: AccountId,
    /// Expected MultiSignature algorithm.
    pub(crate) crypto_type: CryptoType,
    /// Normalized era carrier used by the core encoder.
    pub(crate) era: Value,
    /// Selected pool-aware nonce.
    pub(crate) nonce: u64,
    pub(crate) tip: u64,
    pub(crate) tip_asset_id: Option<u64>,
    /// Immutable chain genesis identity.
    pub(crate) genesis_hash: H256,
    /// Era's finalized-history anchor hash.
    pub(crate) era_block_hash: H256,
    /// RFC-0078 digest, enabling CheckMetadataHash when present.
    pub(crate) metadata_hash: Option<[u8; 32]>,
    /// Already size-normalized signing bytes.
    pub(crate) payload: Bytes,
    /// Signed fields transmitted in the extrinsic.
    pub(crate) included_in_extrinsic: Bytes,
    /// Signed implied fields omitted from the extrinsic.
    pub(crate) included_in_signed_data: Bytes,
    /// Polkadot-JS JSON view of the same transaction.
    pub(crate) payload_json: SignerPayloadJson,
    /// Metadata-verifying signer context.
    pub(crate) context: SigningContext,
    /// Finalized height when this transaction was prepared.
    pub(crate) submission_head: u64,
    /// First height after its last mortal validity window.
    pub(crate) expiry: Option<u64>,
    pub(crate) runtime: RuntimeHandle,
}
impl UnsignedExtrinsic {
    /// Borrow the spec-stamped SCALE call bytes.
    pub fn call(&self) -> &CallBytes {
        &self.call
    }
    /// Expected signing account.
    pub fn address(&self) -> AccountId {
        self.address
    }
    /// Expected MultiSignature algorithm.
    pub fn crypto_type(&self) -> CryptoType {
        self.crypto_type
    }
    /// Borrow the normalized era used by the core encoder.
    pub fn era(&self) -> &Value {
        &self.era
    }
    /// Selected pool-aware nonce.
    pub fn nonce(&self) -> u64 {
        self.nonce
    }
    /// Native TAO tip in rao.
    pub fn tip(&self) -> u64 {
        self.tip
    }
    /// Optional fee asset identifier.
    pub fn tip_asset_id(&self) -> Option<u64> {
        self.tip_asset_id
    }
    /// Immutable chain genesis identity.
    pub fn genesis_hash(&self) -> H256 {
        self.genesis_hash
    }
    /// Era's finalized-history anchor hash.
    pub fn era_block_hash(&self) -> H256 {
        self.era_block_hash
    }
    /// RFC-0078 digest, enabling CheckMetadataHash when present.
    pub fn metadata_hash(&self) -> Option<[u8; 32]> {
        self.metadata_hash
    }
    /// Borrow the already size-normalized signing bytes.
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }
    /// Borrow signed fields transmitted in the extrinsic.
    pub fn included_in_extrinsic(&self) -> &[u8] {
        &self.included_in_extrinsic
    }
    /// Borrow signed implied fields omitted from the extrinsic.
    pub fn included_in_signed_data(&self) -> &[u8] {
        &self.included_in_signed_data
    }
    /// Borrow the Polkadot-JS JSON view of this transaction.
    pub fn payload_json(&self) -> &SignerPayloadJson {
        &self.payload_json
    }
    /// Borrow the metadata-verifying signer context.
    pub fn context(&self) -> &SigningContext {
        &self.context
    }
    /// Finalized height when this transaction was prepared.
    pub fn submission_head(&self) -> u64 {
        self.submission_head
    }
    /// First height after its last mortal validity window.
    pub fn expiry(&self) -> Option<u64> {
        self.expiry
    }
    /// Borrow all prepared signing views without re-encoding.
    pub fn signing_request(&self, purpose: SignPurpose) -> SigningRequest<'_> {
        SigningRequest {
            call: self.call.as_bytes(),
            included_in_extrinsic: &self.included_in_extrinsic,
            included_in_signed_data: &self.included_in_signed_data,
            payload: &self.payload,
            payload_json: &self.payload_json,
            context: &self.context,
            purpose,
        }
    }
    fn params(&self) -> TxParams {
        TxParams {
            era: self.era.clone(),
            nonce: self.nonce,
            tip: self.tip.into(),
            tip_asset_id: self.tip_asset_id.map(u128::from),
            genesis_hash: self.genesis_hash.0,
            era_block_hash: self.era_block_hash.0,
            metadata_hash: self.metadata_hash,
        }
    }
}

/// Observed transaction inclusion state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Inclusion {
    /// No pool acceptance or included block has been confirmed.
    Unobserved,
    /// Pool acceptance before dispatch.
    PoolAccepted,
    /// Resolved inclusion without confirmed finality.
    InBlock,
    /// Resolved finalized inclusion.
    Finalized,
}
/// Resolved chain outcome, preserving Python's pool-acceptance behavior.
#[derive(Clone, Debug)]
pub struct ExtrinsicReport {
    /// Submitted extrinsic identity.
    pub hash: H256,
    /// Dispatch success, or true for pool acceptance before dispatch.
    pub success: bool,
    /// Python-compatible outcome diagnostic.
    pub message: String,
    /// Observed inclusion state.
    pub inclusion: Inclusion,
    /// Inclusion block identity, absent for pool acceptance.
    pub block_hash: Option<H256>,
    /// Inclusion height, absent for pool acceptance.
    pub block_number: Option<u64>,
    /// Index within the inclusion block.
    pub extrinsic_index: Option<u32>,
    /// Explorer index with four-digit position padding.
    pub extrinsic_id: Option<String>,
    /// Actual event-derived fee in checked native rao.
    pub fee: Option<TaoBalance>,
    /// Triggered event records at the block's parent-selected runtime.
    pub events: Vec<crate::EventRecord>,
    /// Metadata-resolved dispatch failure, if any.
    pub error: Option<rtao_core::ops::dispatch_error::DispatchErrorInfo>,
    /// Paired carrier and sealed inner identities for shielded submissions.
    pub shield: Option<crate::shield::ShieldReport>,
}
/// Typed pool rejection with unrecognized diagnostics retained.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PoolRejection {
    /// Runtime custom transaction code.
    Custom(u8),
    /// Nonce is in the future queue.
    Future,
    /// Nonce is outdated.
    Stale,
    /// Replacement priority is insufficient.
    PriorityTooLow,
    /// Extrinsic is already imported.
    AlreadyImported,
    /// Extrinsic is temporarily banned.
    TemporarilyBanned,
    /// Recognized standard invalid-transaction name.
    InvalidTransaction(String),
    /// Terminal watch status diagnostic.
    Watch(String),
    /// Unrecognized pool rejection diagnostic.
    Unknown(String),
}
impl fmt::Display for PoolRejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Custom(code) => write!(f, "Custom error: {code}"),
            Self::Future => f.write_str("Future"),
            Self::Stale => f.write_str("Stale"),
            Self::PriorityTooLow => f.write_str("Priority is too low"),
            Self::AlreadyImported => f.write_str("Already imported"),
            Self::TemporarilyBanned => f.write_str("Transaction is temporarily banned"),
            Self::InvalidTransaction(name) | Self::Watch(name) | Self::Unknown(name) => {
                f.write_str(name)
            }
        }
    }
}
/// Submission failure, distinguishing client errors, pool refusal and fate.
#[derive(Debug)]
#[non_exhaustive]
pub enum SubmitFailure {
    /// Ordinary metadata, encoding, signer or client failure.
    Client(ClientError),
    /// Typed submission transport failure, including unknown fate.
    Submit(rtao_rpc::SubmitError),
    /// Pool or terminal watch rejection.
    Pool(PoolRejection),
    /// Finality lag leaves no valid shield carrier/inner inclusion window.
    ShieldEraExpired {
        /// Selected shield era in blocks.
        period: u64,
        /// Best-head distance from the shared finalized era anchor.
        finality_lag: u64,
    },
    /// A retracted watch ended before a later inclusion could be observed.
    Retracted {
        /// Extrinsic whose finalized history must be reconciled.
        hash: H256,
    },
    /// Mortal transaction absent after finalized history passed its validity.
    Expired {
        /// Expired extrinsic identity.
        hash: H256,
        /// End of the validity window.
        expiry: u64,
    },
}
impl fmt::Display for SubmitFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Client(error) => error.fmt(f),
            Self::Submit(error) => error.fmt(f),
            Self::Pool(error) => error.fmt(f),
            Self::ShieldEraExpired {
                period,
                finality_lag,
            } => write!(
                f,
                "shield era of {period} blocks expires before inclusion with finality lag {finality_lag}"
            ),
            Self::Retracted { hash } => write!(
                f,
                "retracted watch ended for extrinsic {}",
                crate::hash_hex(*hash)
            ),
            Self::Expired { hash, expiry } => write!(
                f,
                "extrinsic {} expired at block {expiry} without inclusion",
                crate::hash_hex(*hash)
            ),
        }
    }
}
impl std::error::Error for SubmitFailure {}
impl From<ClientError> for SubmitFailure {
    fn from(error: ClientError) -> Self {
        Self::Client(error)
    }
}
impl From<CoreError> for SubmitFailure {
    fn from(error: CoreError) -> Self {
        Self::Client(ClientError::from(error))
    }
}

impl Client {
    /// Estimate dispatch weight and fee using public identity only.
    ///
    /// The immortal probe carries a zero signature, no metadata hash and zero
    /// tip. Its fresh nonce query never reserves or mutates cached state.
    pub async fn estimate_weight(
        &self,
        call: CallBytes,
        account: AccountId,
        crypto_type: CryptoType,
    ) -> Result<rtao_core::ops::fee::FeeEstimate, ClientError> {
        self.ensure_signing().await?;
        let runtime = self.refresh_runtime().await?;
        call.ensure_runtime(&runtime).map_err(ClientError::from)?;
        let nonce = self.nonces().fresh(&account).await?;
        let probe = rtao_core::ops::fee::probe(
            runtime.core(),
            call.as_bytes(),
            account.0,
            u8::from(crypto_type),
            nonce,
            self.genesis().0,
        )
        .map_err(CoreError::from)?;
        let request = rtao_core::ops::fee::request(&probe, None).map_err(CoreError::from)?;
        let response: serde_json::Value = self.request(request.method, request.params).await?;
        rtao_core::ops::fee::decode(runtime.core(), &response)
            .map_err(CoreError::from)
            .map_err(ClientError::from)
    }
    /// Sign, assemble and submit using this connection's shared nonce manager.
    pub async fn submit(
        &self,
        call: CallBytes,
        signer: &dyn Signer,
        opts: &SubmitOptions,
    ) -> Result<ExtrinsicReport, SubmitFailure> {
        let limits = SubmissionLimits::new(opts)?;
        let attempted_hash = OnceLock::new();
        let operation = async {
            let lease = limits
                .admission
                .run(self, async {
                    Ok::<_, ClientError>(match opts.nonce {
                        NoncePolicy::Auto => self.nonces().lease(&signer.account()).await?,
                        NoncePolicy::Explicit(nonce) => {
                            self.nonces().lease_explicit(&signer.account(), nonce)
                        }
                    })
                })
                .await?;
            self.submit_with_lease(call, signer, lease, opts, limits, &attempted_hash)
                .await
        };
        limits.full.run(self, operation).await.map_err(|error| {
            submission_timeout_error(self, signer.account(), error, &attempted_hash)
        })
    }

    /// Submit a caller-owned nonce lease, settling only on resolved acceptance.
    pub async fn submit_leased(
        &self,
        call: CallBytes,
        signer: &dyn Signer,
        lease: NonceLease,
        opts: &SubmitOptions,
    ) -> Result<ExtrinsicReport, SubmitFailure> {
        let limits = SubmissionLimits::new(opts)?;
        let attempted_hash = OnceLock::new();
        limits
            .full
            .run(
                self,
                self.submit_with_lease(call, signer, lease, opts, limits, &attempted_hash),
            )
            .await
            .map_err(|error| {
                submission_timeout_error(self, signer.account(), error, &attempted_hash)
            })
    }

    async fn submit_with_lease(
        &self,
        call: CallBytes,
        signer: &dyn Signer,
        lease: NonceLease,
        opts: &SubmitOptions,
        limits: SubmissionLimits,
        attempted_hash: &OnceLock<H256>,
    ) -> Result<ExtrinsicReport, SubmitFailure> {
        if lease.account() != signer.account() {
            return Err(ClientError::Protocol(
                "nonce lease belongs to a different signing account".into(),
            )
            .into());
        }
        let unsigned = limits
            .admission
            .run(
                self,
                self.prepare_at_nonce(call, signer, opts, lease.value()),
            )
            .await?;
        let request = unsigned.signing_request(SignPurpose::Extrinsic);
        let signature = limits
            .admission
            .run(self, async {
                signer.sign(&request).await.map_err(ClientError::Signing)
            })
            .await?;
        let (bytes, hash) = limits
            .admission
            .run(self, self.attach_signature(&unsigned, signature))
            .await?;
        let outcome = self
            .send_and_watch(&bytes, hash, opts, limits.admission, attempted_hash)
            .await;
        match outcome {
            Ok(report) => {
                limits.full.check(self)?;
                lease.settle();
                Ok(report)
            }
            Err(error) => {
                // Python clears immediately on unknown fate. Reconciliation
                // answers whether it landed without keeping a nonce reserved.
                drop(lease);
                let error = submission_timeout_error(self, signer.account(), error, attempted_hash);
                if (opts.reconcile && unknown_fate(&error))
                    || matches!(error, SubmitFailure::Retracted { .. })
                {
                    limits.full.check(self)?;
                    self.reconcile(hash, unsigned.submission_head, unsigned.expiry)
                        .await
                } else {
                    Err(error)
                }
            }
        }
    }

    pub(crate) async fn send_and_watch(
        &self,
        bytes: &[u8],
        hash: H256,
        opts: &SubmitOptions,
        limit: OperationLimit,
        attempted_hash: &OnceLock<H256>,
    ) -> Result<ExtrinsicReport, SubmitFailure> {
        limit.check(self)?;
        let wait = opts.wait;
        let params = json!([format!("0x{}", hex::encode(bytes))]);
        if wait == Wait::Pool {
            let mut attempted = false;
            let response: serde_json::Value = limit
                .run(self, async {
                    attempted = true;
                    let _ = attempted_hash.set(hash);
                    self.request("author_submitExtrinsic", params).await
                })
                .await
                .map_err(|error| author_ack_error(error, hash, attempted))?;
            if crate::parse_hash(&response)? != hash {
                return Err(ClientError::Protocol(
                    "pool returned a different extrinsic hash".into(),
                )
                .into());
            }
            return Ok(ExtrinsicReport {
                hash,
                success: true,
                message: "Submitted (not waiting).".into(),
                inclusion: Inclusion::PoolAccepted,
                block_hash: None,
                block_number: None,
                extrinsic_index: None,
                extrinsic_id: None,
                fee: None,
                events: Vec::new(),
                error: None,
                shield: None,
            });
        }
        let raw = serde_json::value::to_raw_value(&params)
            .map_err(|error| ClientError::Protocol(error.to_string()))?;
        let mut attempted = false;
        let mut subscription = limit
            .run(self, async {
                attempted = true;
                let _ = attempted_hash.set(hash);
                self.rpc()
                    .subscribe_raw(
                        "author_submitAndWatchExtrinsic",
                        Some(raw),
                        "author_unwatchExtrinsic",
                    )
                    .await
                    .map_err(ClientError::from)
            })
            .await
            .map_err(|error| author_ack_error(error, hash, attempted))?;
        let mut retracted = false;
        loop {
            let item = tokio::select! {
                ()=self.closed()=>return Err(ClientError::from(rtao_rpc::RpcError::Closed).into()),
                item=subscription.stream.next()=>item,
            };
            let Some(item) = item else {
                if retracted {
                    return Err(SubmitFailure::Retracted { hash });
                }
                return Err(SubmitFailure::Submit(rtao_rpc::SubmitError::FateUnknown {
                    hash: rtao_rpc::H256(hash.0),
                }));
            };
            let raw = match item {
                Ok(raw) => raw,
                Err(_) if retracted => return Err(SubmitFailure::Retracted { hash }),
                Err(error) => return Err(submission_error(error.into(), hash)),
            };
            let status: serde_json::Value = serde_json::from_str(raw.get())
                .map_err(|error| ClientError::Protocol(error.to_string()))?;
            let normalized = match &status {
                serde_json::Value::String(name) => {
                    (name.to_ascii_lowercase(), serde_json::Value::Null)
                }
                serde_json::Value::Object(fields) if fields.len() == 1 => {
                    let Some((name, value)) = fields.iter().next() else {
                        return Err(
                            ClientError::Protocol("empty transaction watch status".into()).into(),
                        );
                    };
                    (name.to_ascii_lowercase(), value.clone())
                }
                _ => {
                    return Err(
                        ClientError::Protocol("invalid transaction watch status".into()).into(),
                    );
                }
            };
            match normalized.0.as_str() {
                "future" | "ready" | "broadcast" => {}
                "retracted" => {
                    retracted = true;
                }
                "usurped" | "invalid" => {
                    return Err(SubmitFailure::Pool(PoolRejection::Watch(format!(
                        "Extrinsic {}: {}",
                        normalized.0,
                        watch_status_repr(&status)
                    ))));
                }
                "dropped" => {
                    if retracted {
                        return Err(SubmitFailure::Retracted { hash });
                    }
                    if opts.reconcile {
                        return Err(SubmitFailure::Submit(rtao_rpc::SubmitError::FateUnknown {
                            hash: rtao_rpc::H256(hash.0),
                        }));
                    }
                    return Err(SubmitFailure::Pool(PoolRejection::Watch(format!(
                        "Extrinsic dropped: {}",
                        watch_status_repr(&status)
                    ))));
                }
                "inblock" if wait == Wait::Inclusion => {
                    return self
                        .resolve_report(hash, crate::parse_hash(&normalized.1)?, false)
                        .await;
                }
                "finalized" => {
                    return self
                        .resolve_report(hash, crate::parse_hash(&normalized.1)?, true)
                        .await;
                }
                "finalitytimeout" => {
                    let block = crate::parse_hash(&normalized.1)?;
                    let mut report = self.resolve_report(hash, block, false).await?;
                    if wait == Wait::Finalization {
                        self.confirm_finality(
                            hash,
                            block,
                            report.block_number.ok_or_else(|| {
                                ClientError::Protocol("resolved report has no block number".into())
                            })?,
                        )
                        .await?;
                        report.inclusion = Inclusion::Finalized;
                    }
                    return Ok(report);
                }
                "inblock" => {}
                _ => {
                    return Err(ClientError::Protocol(format!(
                        "unknown transaction watch status {} (retracted={retracted})",
                        normalized.0
                    ))
                    .into());
                }
            }
        }
    }

    /// Resolve an included extrinsic through its block's parent-selected runtime.
    pub async fn resolve_extrinsic(
        &self,
        hash: H256,
        block: H256,
        finalized: bool,
    ) -> Result<ExtrinsicReport, SubmitFailure> {
        self.resolve_report(hash, block, finalized).await
    }

    async fn resolve_report(
        &self,
        hash: H256,
        block: H256,
        finalized: bool,
    ) -> Result<ExtrinsicReport, SubmitFailure> {
        let raw: serde_json::Value = self
            .request("chain_getBlock", json!([crate::hash_hex(block)]))
            .await?;
        let number =
            rtao_core::ops::extrinsic_lookup::block_number(&raw).map_err(CoreError::from)?;
        let index = rtao_core::ops::extrinsic_lookup::find(&raw, hash.0)
            .map_err(CoreError::from)?
            .ok_or_else(|| {
                ClientError::Protocol(rtao_core::ops::extrinsic_lookup::missing_message(
                    &crate::hash_hex(hash),
                    &crate::hash_hex(block),
                    number,
                ))
            })?;
        let snapshot = self.at(BlockRef::Hash(block)).await?;
        let events = snapshot.events().await?;
        let records = events.for_extrinsic(index).cloned().collect::<Vec<_>>();
        let values = records
            .iter()
            .map(|record| record.value().clone())
            .collect::<Vec<_>>();
        let runtime = View::runtime(&snapshot).await?;
        let outcome = rtao_core::ops::receipt::resolve_outcome(runtime.core(), &values)
            .map_err(CoreError::from)?;
        let message = if outcome.success {
            "Success".into()
        } else {
            outcome
                .error
                .as_ref()
                .map(rtao_core::ops::dispatch_error::DispatchErrorInfo::result_message)
                .transpose()
                .map_err(CoreError::from)?
                .unwrap_or_else(|| "None".into())
        };
        Ok(ExtrinsicReport {
            hash,
            success: outcome.success,
            message,
            inclusion: if finalized {
                Inclusion::Finalized
            } else {
                Inclusion::InBlock
            },
            block_hash: Some(block),
            block_number: Some(number),
            extrinsic_index: Some(index),
            extrinsic_id: Some(format!("{number}-{index:04}")),
            fee: Some(TaoBalance(outcome.total_fee_amount)),
            events: records,
            error: outcome.error,
            shield: None,
        })
    }

    async fn confirm_finality(
        &self,
        extrinsic_hash: H256,
        hash: H256,
        number: u64,
    ) -> Result<(), SubmitFailure> {
        loop {
            let finalized = self.at(BlockRef::Finalized).await?;
            if finalized.block().number >= number {
                if self.at(BlockRef::Number(number)).await?.block().hash == hash {
                    return Ok(());
                }
                return Err(SubmitFailure::Submit(rtao_rpc::SubmitError::FateUnknown {
                    hash: rtao_rpc::H256(extrinsic_hash.0),
                }));
            }
            self.next_finalized(finalized.block().number).await?;
        }
    }

    async fn next_finalized(&self, previous: u64) -> Result<(), ClientError> {
        if let Some(heads) = self.heads() {
            let mut watch = heads.finalized();
            loop {
                if watch.borrow_and_update().number > previous {
                    return Ok(());
                }
                tokio::select! {()=self.closed()=>return Err(rtao_rpc::RpcError::Closed.into()),changed=watch.changed()=>changed.map_err(|_|ClientError::from(rtao_rpc::RpcError::Closed))?}
            }
        }
        tokio::select! {()=self.closed()=>Err(rtao_rpc::RpcError::Closed.into()),()=tokio::time::sleep(Duration::from_secs(12))=>Ok(())}
    }

    pub(crate) async fn reconcile(
        &self,
        hash: H256,
        start: u64,
        expiry: Option<u64>,
    ) -> Result<ExtrinsicReport, SubmitFailure> {
        let mut next = start;
        loop {
            let finalized = self.at(BlockRef::Finalized).await?;
            let height = finalized.block().number;
            while next <= height && expiry.is_none_or(|expiry| next <= expiry) {
                let block = self.at(BlockRef::Number(next)).await?.block().hash;
                if self.find_extrinsic(hash, block).await?.is_some() {
                    return self.resolve_report(hash, block, true).await;
                }
                next = next.checked_add(1).ok_or_else(|| {
                    ClientError::Protocol("reconciliation height overflow".into())
                })?;
            }
            if let Some(expiry) = expiry
                && height > expiry
            {
                return Err(SubmitFailure::Expired { hash, expiry });
            }
            self.next_finalized(height).await?;
        }
    }
    /// Prepare a call with a fresh nonce without reserving cached state.
    pub async fn prepare_call(
        &self,
        call: CallBytes,
        signer: &dyn Signer,
        opts: &SubmitOptions,
    ) -> Result<UnsignedExtrinsic, ClientError> {
        let limit = SubmissionLimits::new(opts)?.admission;
        let operation = async {
            let nonce = match opts.nonce {
                NoncePolicy::Auto => self.nonces().fresh(&signer.account()).await?,
                NoncePolicy::Explicit(nonce) => nonce,
            };
            self.prepare_at_nonce(call, signer, opts, nonce).await
        };
        limit.run(self, operation).await
    }

    /// Submit a prepared external signature after checking both runtime stamps.
    pub async fn submit_signature(
        &self,
        unsigned: &UnsignedExtrinsic,
        signature: MultiSignature,
        opts: &SubmitOptions,
    ) -> Result<ExtrinsicReport, SubmitFailure> {
        let limits = SubmissionLimits::new(opts)?;
        let attempted_hash = OnceLock::new();
        let operation = async {
            unsigned.verify_signature(signature)?;
            let (bytes, hash) = limits
                .admission
                .run(self, self.attach_signature(unsigned, signature))
                .await?;
            let lease = self
                .nonces()
                .lease_explicit(&unsigned.address, unsigned.nonce);
            match self
                .send_and_watch(&bytes, hash, opts, limits.admission, &attempted_hash)
                .await
            {
                Ok(report) => {
                    limits.full.check(self)?;
                    lease.settle();
                    Ok(report)
                }
                Err(error) => {
                    drop(lease);
                    let error =
                        submission_timeout_error(self, unsigned.address, error, &attempted_hash);
                    if (opts.reconcile && unknown_fate(&error))
                        || matches!(error, SubmitFailure::Retracted { .. })
                    {
                        limits.full.check(self)?;
                        self.reconcile(hash, unsigned.submission_head, unsigned.expiry)
                            .await
                    } else {
                        Err(error)
                    }
                }
            }
        };
        limits.full.run(self, operation).await.map_err(|error| {
            submission_timeout_error(self, unsigned.address, error, &attempted_hash)
        })
    }
    async fn prepare_at_nonce(
        &self,
        call: CallBytes,
        signer: &dyn Signer,
        opts: &SubmitOptions,
        nonce: u64,
    ) -> Result<UnsignedExtrinsic, ClientError> {
        self.prepare_at_nonce_anchored(call, signer, opts, nonce, None)
            .await
    }

    pub(crate) async fn prepare_at_nonce_anchored(
        &self,
        call: CallBytes,
        signer: &dyn Signer,
        opts: &SubmitOptions,
        nonce: u64,
        finalized_height: Option<u64>,
    ) -> Result<UnsignedExtrinsic, ClientError> {
        self.prepare_at_nonce_with_fees(call, signer, opts, nonce, finalized_height, (0, None))
            .await
    }

    pub(crate) async fn prepare_at_nonce_with_fees(
        &self,
        call: CallBytes,
        signer: &dyn Signer,
        opts: &SubmitOptions,
        nonce: u64,
        finalized_height: Option<u64>,
        fees: (u64, Option<u64>),
    ) -> Result<UnsignedExtrinsic, ClientError> {
        self.ensure_signing().await?;
        let runtime = self.refresh_runtime().await?;
        call.ensure_runtime(&runtime).map_err(ClientError::from)?;
        runtime
            .signing_status()
            .map_err(|error| ClientError::SigningDisabled {
                reason: error.to_string(),
            })?;
        let height = match finalized_height {
            Some(height) => height,
            None => self.at(BlockRef::Finalized).await?.block().number,
        };
        if let Mortality::Mortal(period) = opts.mortality {
            let best = self.at(BlockRef::Best).await?.block().number;
            let lag = best.saturating_sub(height);
            #[cfg(feature = "metrics")]
            metrics::gauge!("rtao_finality_lag_blocks").set(lag as f64);
            if lag > period.get() / 2 {
                tracing::warn!(
                    finality_lag_blocks = lag,
                    period = period.get(),
                    "finality lag shortens the transaction validity window"
                );
            }
        }
        let (era, anchor, expiry) = match opts.mortality {
            Mortality::Immortal => (Value::Str("00".into()), self.genesis(), None),
            Mortality::Mortal(period) => {
                let birth = rtao_core::extrinsic::era_birth(period.get(), height);
                let anchor = self.at(BlockRef::Number(birth)).await?.block().hash;
                let expiry = birth.checked_add(period.get()).ok_or_else(|| {
                    ClientError::Protocol("mortal era expiry overflows block number".into())
                })?;
                (
                    Value::record(vec![
                        ("period".into(), Value::Uint(period.get().into())),
                        ("current".into(), Value::Uint(height.into())),
                    ]),
                    anchor,
                    Some(expiry),
                )
            }
        };
        let version: serde_json::Value = self
            .request("state_getRuntimeVersion", json!([null]))
            .await?;
        let spec_name = version
            .get("specName")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_owned();
        check_live_stamp(&version, &call)?;
        let chain_info = ChainInfo {
            spec_version: runtime.spec_version(),
            spec_name,
            base58_prefix: 42,
            decimals: opts.metadata_decimals,
            token_symbol: opts.metadata_symbol.clone(),
        };
        let metadata_hash = if signer.caps().contains(SignerCaps::METADATA_HASH) {
            let runtime = runtime.clone();
            let info = chain_info.clone();
            Some(
                rtao_core::offload(move || runtime.metadata_digest(&info))
                    .await
                    .map_err(|error| ClientError::Protocol(error.to_string()))??,
            )
        } else {
            None
        };
        let params = TxParams {
            era: era.clone(),
            nonce,
            tip: fees.0.into(),
            tip_asset_id: fees.1.map(u128::from),
            genesis_hash: self.genesis().0,
            era_block_hash: anchor.0,
            metadata_hash,
        };
        let (extra, additional) = runtime
            .core()
            .signature_payload_parts(&params)
            .map_err(CoreError::from)?;
        let payload = runtime
            .core()
            .signature_payload(call.as_bytes(), &params)
            .map_err(CoreError::from)?;
        let payload_json = payload_json(&runtime, &call, signer.account(), &params)?;
        let context = SigningContext {
            metadata_bytes: runtime.core().metadata_bytes.clone(),
            chain_info,
            transaction_version: runtime.transaction_version(),
            genesis_hash: self.genesis().0,
        };
        Ok(UnsignedExtrinsic {
            call,
            address: signer.account(),
            crypto_type: signer.crypto_type(),
            era,
            nonce,
            tip: fees.0,
            tip_asset_id: fees.1,
            genesis_hash: self.genesis(),
            era_block_hash: anchor,
            metadata_hash,
            payload: payload.into(),
            included_in_extrinsic: extra.into(),
            included_in_signed_data: additional.into(),
            payload_json,
            context,
            submission_head: height,
            expiry,
            runtime,
        })
    }
    /// Validate both runtime stamps and attach a negotiated signature.
    pub async fn attach_signature(
        &self,
        unsigned: &UnsignedExtrinsic,
        signature: MultiSignature,
    ) -> Result<(Bytes, H256), ClientError> {
        unsigned.verify_signature(signature)?;
        self.ensure_signing().await?;
        let runtime = self.refresh_runtime().await?;
        let version: serde_json::Value = self
            .request("state_getRuntimeVersion", json!([null]))
            .await?;
        check_live_stamp(&version, &unsigned.call)?;
        unsigned
            .call
            .ensure_runtime(&runtime)
            .map_err(ClientError::from)?;
        if unsigned.genesis_hash != self.genesis() {
            return Err(ClientError::SigningDisabled {
                reason: "prepared transaction belongs to a different genesis".into(),
            });
        }
        if let Some(expiry) = unsigned.expiry
            && self.at(BlockRef::Finalized).await?.block().number >= expiry
        {
            return Err(ClientError::SigningDisabled {
                reason: "prepared transaction's mortal era has expired; prepare and sign again"
                    .into(),
            });
        }
        let (bytes, hash) = unsigned
            .runtime
            .core()
            .encode_signed_extrinsic(
                unsigned.call.as_bytes(),
                unsigned.address.0,
                &signature.bytes,
                u8::from(signature.crypto),
                &unsigned.params(),
            )
            .map_err(CoreError::from)?;
        Ok((bytes.into(), H256(hash)))
    }
}

fn check_live_stamp(version: &serde_json::Value, call: &CallBytes) -> Result<(), ClientError> {
    let spec = version
        .get("specVersion")
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| {
            ClientError::Protocol("state_getRuntimeVersion returned invalid specVersion".into())
        })?;
    let tx = version
        .get("transactionVersion")
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| {
            ClientError::Protocol(
                "state_getRuntimeVersion returned invalid transactionVersion".into(),
            )
        })?;
    if spec != call.spec_version() || tx != call.transaction_version() {
        return Err(rtao_runtime::ComposeError::VersionMismatch {
            composed: call.stamp(),
            runtime: rtao_runtime::RuntimeStamp {
                spec_version: spec,
                transaction_version: tx,
            },
        }
        .into());
    }
    Ok(())
}
pub(crate) fn payload_json(
    runtime: &RuntimeHandle,
    call: &CallBytes,
    address: AccountId,
    params: &TxParams,
) -> Result<SignerPayloadJson, ClientError> {
    let nonce = u32::try_from(params.nonce)
        .map_err(|_| ClientError::Protocol("nonce exceeds SignerPayloadJSON u32 width".into()))?;
    let mut era = Vec::new();
    runtime
        .core()
        .encode_era_value(&params.era, &mut era)
        .map_err(CoreError::from)?;
    let mut payload = SignerPayloadJson::new();
    for (name, value) in [
        ("address", json!(address.to_ss58(42))),
        (
            "blockHash",
            json!(crate::hash_hex(H256(params.era_block_hash))),
        ),
        (
            "genesisHash",
            json!(crate::hash_hex(H256(params.genesis_hash))),
        ),
        (
            "method",
            json!(format!("0x{}", hex::encode(call.as_bytes()))),
        ),
        (
            "nonce",
            json!(format!("0x{}", hex::encode(nonce.to_be_bytes()))),
        ),
        (
            "specVersion",
            json!(format!(
                "0x{}",
                hex::encode(runtime.spec_version().to_be_bytes())
            )),
        ),
        (
            "tip",
            json!(format!("0x{}", hex::encode(params.tip.to_be_bytes()))),
        ),
        (
            "transactionVersion",
            json!(format!(
                "0x{}",
                hex::encode(runtime.transaction_version().to_be_bytes())
            )),
        ),
        ("era", json!(format!("0x{}", hex::encode(era)))),
        ("version", json!(runtime.core().extrinsic.version)),
    ] {
        payload.insert(name.into(), value);
    }
    let extensions = runtime
        .core()
        .extrinsic
        .signed_extensions
        .iter()
        .map(|ext| ext.identifier.clone())
        .collect::<Vec<_>>();
    if !extensions.is_empty() {
        payload.insert("signedExtensions".into(), json!(extensions));
    }
    if let Some(asset) = params.tip_asset_id {
        let mut encoded = Vec::new();
        rtao_core::codec::encode::compact(asset, &mut encoded).map_err(CoreError::from)?;
        payload.insert(
            "assetId".into(),
            json!(format!("0x{}", hex::encode(encoded))),
        );
    }
    if let Some(hash) = params.metadata_hash {
        payload.insert("mode".into(), json!(1));
        payload.insert(
            "metadataHash".into(),
            json!(format!("0x{}", hex::encode(hash))),
        );
    } else if extensions.iter().any(|ext| ext == "CheckMetadataHash") {
        payload.insert("mode".into(), json!(0));
        payload.insert("metadataHash".into(), serde_json::Value::Null);
    }
    Ok(payload)
}

#[derive(Clone, Copy)]
pub(crate) struct SubmissionLimits {
    pub(crate) full: OperationLimit,
    pub(crate) admission: OperationLimit,
}
impl SubmissionLimits {
    pub(crate) fn new(opts: &SubmitOptions) -> Result<Self, ClientError> {
        let started = tokio::time::Instant::now();
        let full = OperationLimit::new(started, opts.deadline)?;
        let submit = OperationLimit::new(started, opts.submit_deadline)?;
        let admission = OperationLimit {
            expires: match (full.expires, submit.expires) {
                (Some(full), Some(submit)) => Some(full.min(submit)),
                (full, submit) => full.or(submit),
            },
        };
        Ok(Self { full, admission })
    }
}

#[derive(Clone, Copy)]
pub(crate) struct OperationLimit {
    expires: Option<tokio::time::Instant>,
}
impl OperationLimit {
    fn new(started: tokio::time::Instant, deadline: Option<Duration>) -> Result<Self, ClientError> {
        let expires = deadline
            .map(|deadline| {
                started.checked_add(deadline).ok_or_else(|| {
                    ClientError::Protocol("submission deadline overflows time".into())
                })
            })
            .transpose()?;
        Ok(Self { expires })
    }
    fn timeout() -> ClientError {
        rtao_rpc::RpcError::Timeout {
            detail: "submission timed out without reaching the requested state".into(),
        }
        .into()
    }
    pub(crate) fn check(self, client: &Client) -> Result<(), ClientError> {
        client.ensure_open()?;
        if self
            .expires
            .is_some_and(|deadline| tokio::time::Instant::now() >= deadline)
        {
            return Err(Self::timeout());
        }
        Ok(())
    }
    pub(crate) async fn run<T, E>(
        self,
        client: &Client,
        future: impl std::future::Future<Output = Result<T, E>>,
    ) -> Result<T, E>
    where
        E: From<ClientError>,
    {
        self.check(client)?;
        let operation = async {
            tokio::select! {
                biased;
                () = client.closed() => Err(E::from(rtao_rpc::RpcError::Closed.into())),
                result = future => result,
            }
        };
        let result = match self.expires {
            Some(deadline) => tokio::time::timeout_at(deadline, operation)
                .await
                .map_err(|_| E::from(Self::timeout()))?,
            None => operation.await,
        };
        self.check(client)?;
        result
    }
}

pub(crate) fn unknown_fate(error: &SubmitFailure) -> bool {
    matches!(
        error,
        SubmitFailure::Submit(rtao_rpc::SubmitError::FateUnknown { .. })
    )
}

pub(crate) fn submission_timeout_error(
    client: &Client,
    account: AccountId,
    error: SubmitFailure,
    attempted_hash: &OnceLock<H256>,
) -> SubmitFailure {
    match (&error, attempted_hash.get()) {
        (
            SubmitFailure::Client(ClientError::Rpc(rtao_rpc::RpcError::Timeout { .. })),
            Some(hash),
        ) => {
            // Keep broadcast identity outside the cancelled operation, including
            // while watching, resolving a report or reconciling finalized history.
            // Also invalidate if the final budget check raced with lease settlement.
            client.nonces().clear_account(&account);
            SubmitFailure::Submit(rtao_rpc::SubmitError::FateUnknown {
                hash: rtao_rpc::H256(hash.0),
            })
        }
        _ => error,
    }
}

fn author_ack_error(error: ClientError, hash: H256, attempted: bool) -> SubmitFailure {
    if attempted && matches!(error, ClientError::Rpc(rtao_rpc::RpcError::Timeout { .. })) {
        // The node may have admitted the transaction before its acknowledgment
        // timed out. Clear the lease and optionally reconcile under the full
        // operation budget rather than treating this as a pre-broadcast timeout.
        SubmitFailure::Submit(rtao_rpc::SubmitError::FateUnknown {
            hash: rtao_rpc::H256(hash.0),
        })
    } else {
        submission_error(error, hash)
    }
}

fn submission_error(error: ClientError, hash: H256) -> SubmitFailure {
    match error {
        ClientError::Rpc(rtao_rpc::RpcError::FateUnknown { hash }) => {
            SubmitFailure::Submit(rtao_rpc::SubmitError::FateUnknown { hash })
        }
        ClientError::Rpc(
            rtao_rpc::RpcError::ConnectionLost { .. } | rtao_rpc::RpcError::SubscriptionLost,
        ) => SubmitFailure::Submit(rtao_rpc::SubmitError::FateUnknown {
            hash: rtao_rpc::H256(hash.0),
        }),
        ClientError::Rpc(rtao_rpc::RpcError::JsonRpc {
            code,
            message,
            data,
            ..
        }) => SubmitFailure::Pool(PoolRejection::from_rpc(code, &message, data.as_ref())),
        error => SubmitFailure::Client(error),
    }
}

fn watch_status_repr(status: &serde_json::Value) -> String {
    match status {
        serde_json::Value::String(name) => {
            crate::config::python_repr(&json!({name:serde_json::Value::Null}))
        }
        status => crate::config::python_repr(status),
    }
}

impl PoolRejection {
    /// Resolve structured custom codes, exact node codes, then Python needles.
    pub fn from_rpc(code: i64, message: &str, data: Option<&serde_json::Value>) -> Self {
        use num_traits::ToPrimitive;
        let custom = data
            .and_then(|data| {
                if let Some(value) = data.get("Custom") {
                    python_custom_integer(value)
                } else {
                    match data {
                        serde_json::Value::Bool(_) => python_custom_integer(data),
                        serde_json::Value::Number(value)
                            if !value.as_str().contains(['.', 'e', 'E']) =>
                        {
                            python_custom_integer(data)
                        }
                        serde_json::Value::String(value) => custom_text(value),
                        _ => None,
                    }
                }
            })
            .or_else(|| custom_text(message));
        if let Some(custom) = custom.as_ref().and_then(ToPrimitive::to_u8)
            && rtao_catalog::errors::custom_transaction_name(custom).is_some()
        {
            return Self::Custom(custom);
        }
        // An unmapped custom code keeps its original diagnostic and falls
        // through to watch/standard names, just as Python's resolver does.
        if custom.is_none() {
            match rtao_catalog::rt::POOL_RPC_CODES
                .iter()
                .find_map(|(candidate, name)| (*candidate == code).then_some(*name))
            {
                Some("TemporarilyBanned") => return Self::TemporarilyBanned,
                Some("AlreadyImported") => return Self::AlreadyImported,
                Some("TooLowPriority") => return Self::PriorityTooLow,
                Some("FutureTx") => return Self::Future,
                _ => {}
            }
        }
        let message = if message.is_empty() {
            "Request failed"
        } else {
            message
        };
        let text = match data.filter(|data| !data.is_null()) {
            Some(serde_json::Value::String(data)) => format!("{message}: {data}"),
            Some(data) => format!("{message}: {}", crate::config::python_repr(data)),
            _ => message.into(),
        };
        if watch_name(&text).is_some() {
            return Self::Watch(text);
        }
        match rtao_catalog::errors::invalid_transaction_name(&text) {
            Some("Future") => Self::Future,
            Some("Stale") => Self::Stale,
            Some("PriorityTooLow") => Self::PriorityTooLow,
            Some("AlreadyImported") => Self::AlreadyImported,
            Some(name) => Self::InvalidTransaction(name.into()),
            None => Self::Unknown(text),
        }
    }
    /// Semantic Python error name, when this refusal is recognized.
    pub fn name(&self) -> Option<&str> {
        match self {
            Self::Custom(code) => rtao_catalog::errors::custom_transaction_name(*code),
            Self::Future => Some("Future"),
            Self::Stale => Some("Stale"),
            Self::PriorityTooLow => Some("PriorityTooLow"),
            Self::AlreadyImported => Some("AlreadyImported"),
            Self::TemporarilyBanned => Some("TemporarilyBanned"),
            Self::InvalidTransaction(name) => Some(name),
            Self::Watch(message) => watch_name(message),
            Self::Unknown(_) => None,
        }
    }
}

fn custom_text(text: &str) -> Option<num_bigint::BigInt> {
    static PATTERN: std::sync::OnceLock<Result<regex::Regex, regex::Error>> =
        std::sync::OnceLock::new();
    let pattern = PATTERN
        .get_or_init(|| {
            regex::RegexBuilder::new(rtao_catalog::py::CUSTOM_ERROR_RE)
                .case_insensitive(true)
                .build()
        })
        .as_ref()
        .ok()?;
    crate::config::parse_cache_key(pattern.captures(text)?.get(1)?.as_str())
}

fn python_custom_integer(value: &serde_json::Value) -> Option<num_bigint::BigInt> {
    use num_traits::FromPrimitive;
    match value {
        serde_json::Value::Bool(value) => Some(u8::from(*value).into()),
        serde_json::Value::String(value) => crate::config::parse_cache_key(value),
        serde_json::Value::Number(value) => {
            let spelling = value.as_str();
            if spelling.contains(['.', 'e', 'E']) {
                num_bigint::BigInt::from_f64(value.as_f64()?.trunc())
            } else {
                spelling.parse().ok()
            }
        }
        _ => None,
    }
}

fn watch_name(text: &str) -> Option<&'static str> {
    static PATTERN: std::sync::OnceLock<Result<regex::Regex, regex::Error>> =
        std::sync::OnceLock::new();
    let pattern = PATTERN
        .get_or_init(|| {
            regex::RegexBuilder::new(rtao_catalog::py::WATCH_STATUS_RE)
                .case_insensitive(true)
                .build()
        })
        .as_ref()
        .ok()?;
    let status = pattern.captures(text)?.get(1)?.as_str().to_lowercase();
    rtao_catalog::py::WATCH_STATUS_NAMES
        .iter()
        .find_map(|(name, resolved)| (*name == status).then_some(*resolved))
}
