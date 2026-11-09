// Derived from RaoFoundation/subtensor sdk/python/bittensor/_transport/interface.py @ b6a81d144078eac18bd25c3af7cb31579bb670f3 (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/_transport/contract.py @ b6a81d144078eac18bd25c3af7cb31579bb670f3 (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/_transport/extrinsics.py @ b6a81d144078eac18bd25c3af7cb31579bb670f3 (Apache-2.0); modified by rtao.
//! Hashed and post-quantum signing from chain-read authority (D-147).
//!
//! A rotating account's key changes with every authorized transaction, so
//! its current generation and commitment are always read from chain state,
//! never from a local counter, and its nonces are never pipelined. A standard
//! post-quantum account has one fixed key and pipelines like a classical
//! account.

use std::sync::atomic::Ordering;

use bytes::Bytes;
use rtao_core::{CoreError, extrinsic::TxParams};
use rtao_primitives::{
    AccountId, AuthorityType, H256,
    receiving::{descriptor_bytes, is_stateless_pq_descriptor, raw_bytes},
};
use rtao_runtime::{CallBytes, RuntimeHandle, storage as st};
use rtao_signer::{AuthoritySigner, AuthorizationRequest, SignPurpose};

use super::{
    ExtrinsicReport, NoncePolicy, SubmissionAttempt, SubmissionLimits, SubmitFailure,
    SubmitOptions, check_live_stamp, submission_attempt_error, unknown_fate,
};
use crate::{BlockRef, Client, ClientError, ViewExt};

/// Python `HashedSigningSnapshot`: one authority state at one block.
///
/// A Shield pair signs both of its proofs from one snapshot, so the carrier
/// (generation `g`, nonce `n`) and the inner transaction (generation `g + 1`,
/// nonce `n + 1`) agree on the authority they rotate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HashedSigningSnapshot {
    /// The block the authority was read at.
    pub block_hash: H256,
    /// The account.
    pub account: AccountId,
    /// The account's signing scheme.
    pub authority_type: AuthorityType,
    /// The registered (or, for a standard post-quantum account, its own)
    /// public descriptor.
    pub descriptor: Vec<u8>,
    /// The generation that signs next; zero for a standard post-quantum account.
    pub generation: u64,
    /// The chain's commitment to that generation's key.
    pub commitment: Vec<u8>,
    /// The confirmed account nonce at `block_hash`, or for a standard
    /// post-quantum account its reserved nonce.
    pub nonce: u64,
}

/// A signed hashed or post-quantum extrinsic and its validity window.
#[derive(Clone, Debug)]
pub struct AuthorizedExtrinsic {
    /// The encoded, length-prefixed extrinsic.
    pub bytes: Bytes,
    /// Its blake2-256 hash.
    pub hash: H256,
    /// The signing account.
    pub account: AccountId,
    /// The nonce it uses.
    pub nonce: u64,
    /// The finalized height its era is anchored at.
    pub submission_head: u64,
    /// The last block its mortal era is valid in.
    pub expiry: Option<u64>,
}

fn refused(reason: impl Into<String>) -> ClientError {
    ClientError::SigningDisabled {
        reason: reason.into(),
    }
}

/// The runtime supports this scheme (Python's constant and profile checks).
fn check_scheme(runtime: &RuntimeHandle, kind: AuthorityType) -> Result<(), ClientError> {
    let core = runtime.core();
    match kind.profile() {
        None => {
            if core
                .constant("HashedAccounts", "RegistrationDeposit")
                .is_none()
            {
                return Err(refused("runtime does not support hashed accounts"));
            }
        }
        Some(profile) => {
            if !core.supports_mldsa_profile(profile) {
                return Err(refused(format!("runtime does not support {kind} accounts")));
            }
        }
    }
    Ok(())
}

impl Client {
    /// Python `_hashed_snapshot_at`: the signer's authority at `block`.
    ///
    /// Refuses unless the runtime supports the scheme and
    /// `AdminUtils.HashedAccountsEnabled` is true. A rotating account must be
    /// registered with exactly the signer's descriptor and a well-formed
    /// generation and commitment; a standard post-quantum account's authority
    /// is its own descriptor at generation zero.
    pub async fn hashed_snapshot(
        &self,
        signer: &dyn AuthoritySigner,
        block: BlockRef,
    ) -> Result<HashedSigningSnapshot, ClientError> {
        let kind = signer.authority_type();
        let account = signer.account();
        let view = self.at(block).await?;
        check_scheme(&view.block().runtime, kind)?;
        if !view
            .storage(&st::admin_utils::HASHED_ACCOUNTS_ENABLED, ())
            .await?
        {
            return Err(refused(if kind.is_stateless() {
                "post-quantum authorization is disabled on this chain"
            } else {
                "hashed accounts are not enabled on this chain"
            }));
        }
        let nonce = confirmed_nonce(&view, account).await?;
        let block_hash = view.block().hash;
        if kind.is_stateless() {
            let descriptor = signer.descriptor().to_vec();
            if !is_stateless_pq_descriptor(&descriptor) || Some(descriptor[1]) != kind.profile() {
                return Err(refused(
                    "standard post-quantum signing snapshot does not match this wallet",
                ));
            }
            return Ok(HashedSigningSnapshot {
                block_hash,
                account,
                authority_type: kind,
                commitment: descriptor[2..].to_vec(),
                descriptor,
                generation: 0,
                nonce,
            });
        }
        let record = if kind.is_post_quantum() {
            view.storage(&st::hashed_accounts::ML_DSA_ACCOUNTS, (account,))
                .await?
        } else {
            view.storage(&st::hashed_accounts::ACCOUNTS, (account,))
                .await?
        };
        let Some(record) = record else {
            return Err(refused(
                "hashed account is not registered; fund or swap to its local wallet name from an existing wallet, or have a sponsor register its public descriptor",
            ));
        };
        let record = record.to_json();
        let mismatch = || refused("registered hashed account does not match this wallet");
        let descriptor = record
            .get("descriptor")
            .map(descriptor_bytes)
            .transpose()
            .map_err(|_| mismatch())?
            .ok_or_else(mismatch)?;
        if descriptor != signer.descriptor() {
            return Err(mismatch());
        }
        let generation = record
            .get("generation")
            .and_then(serde_json::Value::as_u64)
            .filter(|generation| *generation < u64::MAX)
            .ok_or_else(mismatch)?;
        let commitment = record
            .get("commitment")
            .and_then(raw_bytes)
            .filter(|commitment| commitment.len() == kind.commitment_length())
            .ok_or_else(mismatch)?;
        Ok(HashedSigningSnapshot {
            block_hash,
            account,
            authority_type: kind,
            descriptor,
            generation,
            commitment,
            nonce,
        })
    }

    /// Python `prepare_hashed_snapshot`: finalized authority for a Shield pair.
    ///
    /// A rotating account must have no pending transaction. A standard
    /// post-quantum account's nonce is the node's next index (Python's
    /// `reserve_nonces=False`); callers that submit reserve it with a lease.
    pub async fn prepare_hashed_snapshot(
        &self,
        signer: &dyn AuthoritySigner,
    ) -> Result<HashedSigningSnapshot, ClientError> {
        let mut snapshot = self.hashed_snapshot(signer, BlockRef::Finalized).await?;
        let pending = self.nonces().fresh(&snapshot.account).await?;
        if snapshot.authority_type.is_stateless() {
            snapshot.nonce = pending;
        } else if pending != snapshot.nonce {
            return Err(refused(
                "hashed account has a pending transaction; wait for finalization before preparing Shield",
            ));
        }
        Ok(snapshot)
    }

    /// Sign `call` from `snapshot`, at its generation and nonce plus
    /// `offset` (0, or 1 for a Shield inner transaction).
    ///
    /// The signer's identity and, for a rotating account, its key's
    /// commitment must equal the snapshot's; the proof is verified against
    /// that authority before the core encodes the extrinsic.
    pub async fn sign_authorized(
        &self,
        call: CallBytes,
        signer: &dyn AuthoritySigner,
        opts: &SubmitOptions,
        snapshot: &HashedSigningSnapshot,
        offset: u64,
    ) -> Result<AuthorizedExtrinsic, ClientError> {
        let kind = signer.authority_type();
        if offset > 1 {
            return Err(refused(
                "hashed generation offset must be zero or one for a Shield inner call",
            ));
        }
        if snapshot.account != signer.account()
            || snapshot.authority_type != kind
            || snapshot.descriptor != signer.descriptor()
        {
            return Err(refused(
                "hashed signing snapshot does not match the pinned account authority",
            ));
        }
        let nonce = snapshot
            .nonce
            .checked_add(offset)
            .filter(|nonce| !kind.is_stateless() || *nonce < u64::from(u32::MAX))
            .ok_or_else(|| refused("standard post-quantum nonce is exhausted or invalid"))?;
        let (generation, commitment) = if kind.is_stateless() {
            if snapshot.generation != 0 || snapshot.commitment != snapshot.descriptor[2..] {
                return Err(refused(
                    "standard post-quantum signing snapshot does not match this wallet",
                ));
            }
            (0, snapshot.commitment.clone())
        } else {
            if snapshot.generation >= u64::MAX - 1 - offset {
                return Err(refused("hashed signing generation is exhausted"));
            }
            let current = signer
                .commitment(snapshot.generation)
                .map_err(ClientError::Signing)?;
            if current != snapshot.commitment {
                return Err(refused(
                    "derived hashed signing key does not match the current on-chain commitment",
                ));
            }
            let generation = snapshot.generation + offset;
            let commitment = if offset == 0 {
                current
            } else {
                signer
                    .commitment(generation)
                    .map_err(ClientError::Signing)?
            };
            (generation, commitment)
        };
        self.ensure_signing_network()?;
        let (runtime, version) = self.refresh_runtime_version().await?;
        call.ensure_runtime(&runtime).map_err(ClientError::from)?;
        runtime
            .signing_status()
            .map_err(|error| refused(error.to_string()))?;
        check_live_stamp(&version, &call)?;
        check_scheme(&runtime, kind)?;
        let height = self.finalized_number().await?;
        let (era, anchor, expiry) = self.era_for(opts.mortality, height).await?;
        let params = TxParams {
            era,
            nonce,
            tip: 0,
            tip_asset_id: None,
            genesis_hash: self.genesis().0,
            era_block_hash: anchor.0,
            metadata_hash: None,
        };
        let (extra, additional) = runtime
            .core()
            .signature_payload_parts(&params)
            .map_err(CoreError::from)?;
        // The complete payload: authorization never uses the 256-byte hash.
        let mut payload =
            Vec::with_capacity(call.as_bytes().len() + extra.len() + additional.len());
        payload.extend_from_slice(call.as_bytes());
        payload.extend_from_slice(&extra);
        payload.extend_from_slice(&additional);
        let request = AuthorizationRequest {
            payload: &payload,
            call: call.as_bytes(),
            generation,
            purpose: if offset == 0 {
                SignPurpose::Extrinsic
            } else {
                SignPurpose::ShieldInner
            },
        };
        let proof = signer
            .sign_authorization(&request)
            .await
            .map_err(ClientError::Signing)?;
        proof.check(kind).map_err(ClientError::Signing)?;
        verify_proof(
            kind,
            &proof.bytes,
            &payload,
            &snapshot.descriptor,
            generation,
            &commitment,
        )?;
        let (bytes, hash) = runtime
            .core()
            .encode_signed_extrinsic(
                call.as_bytes(),
                snapshot.account.0,
                &proof.bytes,
                kind.code(),
                &params,
            )
            .map_err(CoreError::from)?;
        Ok(AuthorizedExtrinsic {
            bytes: bytes.into(),
            hash: H256(hash),
            account: snapshot.account,
            nonce,
            submission_head: height,
            expiry,
        })
    }

    /// Sign and submit `call` for a hashed or post-quantum account.
    ///
    /// A rotating account reads its authority at the best block (classical
    /// hashed) or the finalized block (post-quantum), refuses while one of its
    /// transactions is pending and uses the confirmed nonce; an explicit
    /// nonce must equal it. A standard post-quantum account reads finalized
    /// authority and takes an ordinary nonce lease. A journal is refused: it
    /// does not yet record authorized submissions.
    pub async fn submit_authorized(
        &self,
        call: CallBytes,
        signer: &dyn AuthoritySigner,
        opts: &SubmitOptions,
    ) -> Result<ExtrinsicReport, SubmitFailure> {
        if opts.journal.is_some() {
            return Err(refused(
                "the submission journal does not record hashed or post-quantum transactions yet",
            )
            .into());
        }
        let limits = SubmissionLimits::new(opts)?;
        let attempted_hash = SubmissionAttempt::default();
        let account = signer.account();
        let operation = async {
            let (lease, signed) = limits
                .admission
                .run(self, async {
                    let kind = signer.authority_type();
                    let block = if kind.is_post_quantum() {
                        BlockRef::Finalized
                    } else {
                        BlockRef::Best
                    };
                    let mut snapshot = self.hashed_snapshot(signer, block).await?;
                    let lease = if kind.is_stateless() {
                        let lease = match opts.nonce {
                            NoncePolicy::Auto => self.nonces().lease(&account).await?,
                            NoncePolicy::Explicit(nonce) => {
                                self.nonces().lease_explicit(&account, nonce)
                            }
                        };
                        snapshot.nonce = lease.value();
                        lease
                    } else {
                        if self.nonces().fresh(&account).await? != snapshot.nonce {
                            return Err(refused(
                                "hashed account has a pending transaction; wait for finalization before signing again",
                            ));
                        }
                        if matches!(opts.nonce, NoncePolicy::Explicit(nonce) if nonce != snapshot.nonce)
                        {
                            return Err(refused(
                                "hashed accounts must use the current on-chain nonce; refresh and sign again",
                            ));
                        }
                        self.nonces().lease_explicit(&account, snapshot.nonce)
                    };
                    let signed = self.sign_authorized(call, signer, opts, &snapshot, 0).await?;
                    Ok::<_, ClientError>((lease, signed))
                })
                .await?;
            match self
                .send_and_watch(
                    &signed.bytes,
                    signed.hash,
                    opts,
                    limits.admission,
                    &attempted_hash,
                    None,
                )
                .await
            {
                Ok(report) => {
                    limits.full.check(self)?;
                    lease.settle();
                    Ok(report)
                }
                Err(error) => {
                    drop(lease);
                    let error = submission_attempt_error(self, account, error, &attempted_hash);
                    if (opts.reconcile && unknown_fate(&error))
                        || attempted_hash.retracted.load(Ordering::Acquire)
                    {
                        limits.full.check(self)?;
                        self.reconcile(signed.hash, signed.submission_head, signed.expiry)
                            .await
                    } else {
                        Err(error)
                    }
                }
            }
        };
        limits
            .full
            .run(self, operation)
            .await
            .map_err(|error| submission_attempt_error(self, account, error, &attempted_hash))
    }
}

async fn confirmed_nonce(view: &crate::Snapshot, account: AccountId) -> Result<u64, ClientError> {
    let info = view.storage(&st::system::ACCOUNT, (account,)).await?;
    Ok(info
        .to_json()
        .get("nonce")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0))
}

/// Verify a proof against the chain-read authority, never one the proof names.
fn verify_proof(
    kind: AuthorityType,
    proof: &[u8],
    payload: &[u8],
    descriptor: &[u8],
    generation: u64,
    commitment: &[u8],
) -> Result<(), ClientError> {
    let valid = if kind.is_post_quantum() {
        let commitment: &[u8; 64] = commitment
            .try_into()
            .map_err(|_| refused("post-quantum commitment must contain 64 bytes"))?;
        rtao_core::keys::verify_mldsa_authorization(
            proof, payload, descriptor, generation, commitment,
        )
    } else {
        let commitment: &[u8; 32] = commitment
            .try_into()
            .map_err(|_| refused("hashed key commitments must contain 32 bytes"))?;
        rtao_core::keys::verify_hashed_authorization(
            proof, payload, descriptor, generation, commitment,
        )
    }
    .map_err(CoreError::from)?;
    if !valid {
        return Err(ClientError::Signing(
            rtao_signer::SignerError::InvalidSignature(
                "authorization proof does not verify against the account's on-chain authority"
                    .into(),
            ),
        ));
    }
    Ok(())
}
