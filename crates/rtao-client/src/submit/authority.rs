// Derived from RaoFoundation/subtensor sdk/python/bittensor/_transport/interface.py @ b6a81d144078eac18bd25c3af7cb31579bb670f3 (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/_transport/contract.py @ b6a81d144078eac18bd25c3af7cb31579bb670f3 (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/_transport/extrinsics.py @ b6a81d144078eac18bd25c3af7cb31579bb670f3 (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/mldsa_signing.py @ b6a81d144078eac18bd25c3af7cb31579bb670f3 (Apache-2.0); modified by rtao.
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
    /// authority and takes an ordinary nonce lease. A journal is refused
    /// here; [`Client::submit`] with the signer as a [`rtao_signer::Signer`]
    /// (its `authority()` set) records journaled submissions.
    pub async fn submit_authorized(
        &self,
        call: CallBytes,
        signer: &dyn AuthoritySigner,
        opts: &SubmitOptions,
    ) -> Result<ExtrinsicReport, SubmitFailure> {
        if opts.journal.is_some() {
            return Err(refused(
                "submit_authorized does not journal; use Client::submit with the signer as a Signer",
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

/// The signing scheme a fee probe prices: a classical 64-byte signature or a
/// hashed or post-quantum authorization proof of its scheme's length.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProbeSigner<'a> {
    /// An Ed25519 or Sr25519 signature.
    Classical(rtao_primitives::CryptoType),
    /// A hashed or post-quantum account's proof (Python's placeholder proof).
    Authority {
        /// The account's scheme.
        kind: AuthorityType,
        /// Its public descriptor.
        descriptor: &'a [u8],
    },
}

impl ProbeSigner<'_> {
    /// The scheme a signer signs with, from its public identity only.
    pub fn of(signer: &dyn rtao_signer::Signer) -> ProbeSigner<'_> {
        match signer.authority() {
            Some(authority) => ProbeSigner::Authority {
                kind: authority.authority_type(),
                descriptor: authority.descriptor(),
            },
            None => ProbeSigner::Classical(signer.crypto_type()),
        }
    }
    /// The placeholder signature bytes and signature version for `account`.
    pub(crate) fn placeholder(&self, account: AccountId) -> (Vec<u8>, u8) {
        match *self {
            Self::Classical(crypto) => (vec![0; 64], u8::from(crypto)),
            Self::Authority { kind, descriptor } => {
                (kind.placeholder_proof(account, descriptor, 0), kind.code())
            }
        }
    }
}

impl Client {
    /// The authority snapshot and nonce lease for a direct submission
    /// (Python's `_sign_hashed` and stateless reservation).
    ///
    /// A rotating account reads its authority at the best block (classical
    /// hashed) or finalized block (post-quantum), refuses while one of its
    /// transactions is pending and signs at the confirmed nonce; an explicit
    /// nonce must equal it. A standard post-quantum account reads finalized
    /// authority and takes an ordinary nonce lease.
    pub(crate) async fn authority_lease(
        &self,
        signer: &dyn AuthoritySigner,
        nonce: NoncePolicy,
    ) -> Result<(crate::NonceLease, HashedSigningSnapshot), ClientError> {
        let kind = signer.authority_type();
        let account = signer.account();
        let block = if kind.is_post_quantum() {
            BlockRef::Finalized
        } else {
            BlockRef::Best
        };
        let mut snapshot = self.hashed_snapshot(signer, block).await?;
        let lease = if kind.is_stateless() {
            let lease = match nonce {
                NoncePolicy::Auto => self.nonces().lease(&account).await?,
                NoncePolicy::Explicit(nonce) => self.nonces().lease_explicit(&account, nonce),
            };
            snapshot.nonce = lease.value();
            lease
        } else {
            if self.nonces().fresh(&account).await? != snapshot.nonce {
                return Err(refused(
                    "hashed account has a pending transaction; wait for finalization before signing again",
                ));
            }
            if matches!(nonce, NoncePolicy::Explicit(nonce) if nonce != snapshot.nonce) {
                return Err(refused(
                    "hashed accounts must use the current on-chain nonce; refresh and sign again",
                ));
            }
            self.nonces().lease_explicit(&account, snapshot.nonce)
        };
        Ok((lease, snapshot))
    }

    /// The authority snapshot and the adjacent carrier and inner nonce leases
    /// of a Shield pair (Python's `prepare_hashed_snapshot(reserve_nonces=…)`).
    ///
    /// Both proofs come from one finalized snapshot: the carrier at nonce `n`
    /// and generation `g`, the inner at `n + 1` and `g + 1` (a standard
    /// post-quantum account stays at generation zero). A rotating account
    /// must have no pending transaction; a standard post-quantum account
    /// leases the pair like a classical one.
    pub(crate) async fn authority_pair(
        &self,
        signer: &dyn AuthoritySigner,
    ) -> Result<
        (
            (crate::NonceLease, crate::NonceLease),
            bool,
            HashedSigningSnapshot,
        ),
        ClientError,
    > {
        let account = signer.account();
        if signer.authority_type().is_stateless() {
            let (pair, waited) = self.nonces().lease_pair_waited(&account).await?;
            let mut snapshot = self.hashed_snapshot(signer, BlockRef::Finalized).await?;
            snapshot.nonce = pair.0.value();
            return Ok((pair, waited, snapshot));
        }
        let snapshot = self.prepare_hashed_snapshot(signer).await?;
        let nonce = snapshot.nonce;
        let inner = nonce
            .checked_add(1)
            .ok_or_else(|| ClientError::Protocol("account nonce overflow".into()))?;
        let carrier = self.nonces().lease_explicit(&account, nonce);
        let inner = self.nonces().lease_explicit(&account, inner);
        Ok(((carrier, inner), false, snapshot))
    }

    /// Authorize a prepared transaction from `snapshot` at its nonce plus
    /// `offset` (0, or 1 for a Shield inner transaction) and encode it.
    ///
    /// The prepared address and nonce must be the snapshot's, the signer's
    /// key must match the chain's commitment, and the proof is verified
    /// against the chain-read authority before encoding, after the same live
    /// runtime, genesis and era checks as [`Client::attach_signature`].
    pub(crate) async fn authorize_prepared(
        &self,
        unsigned: &crate::UnsignedExtrinsic,
        signer: &dyn AuthoritySigner,
        snapshot: &HashedSigningSnapshot,
        offset: u64,
        purpose: SignPurpose,
    ) -> Result<(Bytes, H256), ClientError> {
        let kind = signer.authority_type();
        if offset > 1 {
            return Err(refused(
                "hashed generation offset must be zero or one for a Shield inner call",
            ));
        }
        if snapshot.account != signer.account()
            || unsigned.address() != snapshot.account
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
        if unsigned.nonce() != nonce {
            return Err(refused(
                "Shield nonce does not match its pinned signing snapshot",
            ));
        }
        let (generation, commitment) = self.authority_generation(signer, snapshot, offset)?;
        self.ensure_signing_network()?;
        let (runtime, version) = self.refresh_runtime_version().await?;
        runtime
            .signing_status()
            .map_err(|error| refused(error.to_string()))?;
        check_live_stamp(&version, unsigned.call())?;
        unsigned
            .call()
            .ensure_runtime(&runtime)
            .map_err(ClientError::from)?;
        check_scheme(&runtime, kind)?;
        if unsigned.genesis_hash() != self.genesis() {
            return Err(refused(
                "prepared transaction belongs to a different genesis",
            ));
        }
        if let Some(expiry) = unsigned.expiry()
            && self.finalized_number().await? >= expiry
        {
            return Err(refused(
                "prepared transaction's mortal era has expired; prepare and sign again",
            ));
        }
        let call = unsigned.call().as_bytes();
        let extra = unsigned.included_in_extrinsic();
        let additional = unsigned.included_in_signed_data();
        // The complete payload: authorization never uses the 256-byte hash.
        let mut payload = Vec::with_capacity(call.len() + extra.len() + additional.len());
        payload.extend_from_slice(call);
        payload.extend_from_slice(extra);
        payload.extend_from_slice(additional);
        let request = AuthorizationRequest {
            payload: &payload,
            call,
            generation,
            purpose,
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
                call,
                snapshot.account.0,
                &proof.bytes,
                kind.code(),
                &unsigned.params(),
            )
            .map_err(CoreError::from)?;
        Ok((bytes.into(), H256(hash)))
    }

    /// The generation and commitment a proof at `offset` signs with, after
    /// checking the signer's key against the snapshot's commitment.
    fn authority_generation(
        &self,
        signer: &dyn AuthoritySigner,
        snapshot: &HashedSigningSnapshot,
        offset: u64,
    ) -> Result<(u64, Vec<u8>), ClientError> {
        if signer.authority_type().is_stateless() {
            if snapshot.generation != 0 || snapshot.commitment != snapshot.descriptor[2..] {
                return Err(refused(
                    "standard post-quantum signing snapshot does not match this wallet",
                ));
            }
            return Ok((0, snapshot.commitment.clone()));
        }
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
        Ok((generation, commitment))
    }
}

impl Client {
    /// Sign a prepared transaction under `admission`: classically (the
    /// signature and its attachment as separate bounded steps), or for a
    /// hashed or post-quantum signer from `snapshot` at `offset` (D-149).
    pub(crate) async fn sign_prepared_as(
        &self,
        admission: super::OperationLimit,
        unsigned: &crate::UnsignedExtrinsic,
        signer: &dyn rtao_signer::Signer,
        snapshot: Option<&HashedSigningSnapshot>,
        offset: u64,
        purpose: SignPurpose,
    ) -> Result<(Bytes, H256), ClientError> {
        match (signer.authority(), snapshot) {
            (Some(authority), Some(snapshot)) => {
                admission
                    .run(
                        self,
                        self.authorize_prepared(unsigned, authority, snapshot, offset, purpose),
                    )
                    .await
            }
            (None, None) => {
                let request = unsigned.signing_request(purpose);
                let signature = admission
                    .run(self, async {
                        signer.sign(&request).await.map_err(ClientError::Signing)
                    })
                    .await?;
                admission
                    .run(self, self.attach_signature(unsigned, signature))
                    .await
            }
            _ => Err(refused("hashed signing snapshot does not match the signer")),
        }
    }
}

/// Python `mldsa_signing.FORMAT`: the ML-DSA offline request format.
pub const MLDSA_REQUEST_FORMAT: &str = "bittensor/ms/v2/extrinsic/1";
/// Python `mldsa_signing.PQ_FORMAT`: the Falcon offline request format.
pub const PQ_REQUEST_FORMAT: &str = "bittensor/pq/extrinsic/1";
const MAX_NATIVE_PAYLOAD: usize = 16 * 1024 * 1024;

/// A public-only view of a post-quantum account's authority, for reads.
struct PublicAuthority {
    account: AccountId,
    kind: AuthorityType,
    descriptor: Vec<u8>,
}

impl AuthoritySigner for PublicAuthority {
    fn account(&self) -> AccountId {
        self.account
    }
    fn authority_type(&self) -> AuthorityType {
        self.kind
    }
    fn descriptor(&self) -> &[u8] {
        &self.descriptor
    }
    fn commitment(&self, _generation: u64) -> Result<Vec<u8>, rtao_signer::SignerError> {
        Err(rtao_signer::SignerError::KeyUnavailable(
            rtao_signer::SignerRole::Coldkey,
        ))
    }
    fn sign_authorization<'a>(
        &'a self,
        _request: &'a AuthorizationRequest<'a>,
    ) -> rtao_signer::BoxFuture<'a, Result<rtao_signer::AuthorizationProof, rtao_signer::SignerError>>
    {
        Box::pin(async {
            Err(rtao_signer::SignerError::KeyUnavailable(
                rtao_signer::SignerRole::Coldkey,
            ))
        })
    }
}

/// Python `PreparedMldsaExtrinsic` (`PreparedPqExtrinsic`): a versioned
/// full-payload request for an offline or device post-quantum signer.
///
/// It carries the complete native payload (never the 256-byte hash) and the
/// authority snapshot it signs at. A request is not proof that its snapshot
/// is authentic: [`sign_prepared_pq`] rebuilds the payload from trusted
/// metadata, and [`Client::submit_pq_proof`] rechecks the current authority.
#[derive(Clone, Debug, PartialEq, Eq)]
#[doc(alias = "bittensor.mldsa_signing.PreparedMldsaExtrinsic")]
pub struct PreparedPqExtrinsic {
    /// SCALE call bytes.
    pub call_data: Vec<u8>,
    /// The signing account's authority at preparation.
    pub authority: HashedSigningSnapshot,
    /// The era as the codec takes it (`"00"` or `{period, current}`).
    pub era: serde_json::Value,
    /// Native TAO tip in rao.
    pub tip: u64,
    /// Optional fee asset.
    pub tip_asset_id: Option<u64>,
    /// The chain's genesis hash.
    pub genesis_hash: H256,
    /// The era's anchor block hash.
    pub era_block_hash: H256,
    /// Runtime spec version at preparation.
    pub spec_version: u32,
    /// Runtime transaction version at preparation.
    pub transaction_version: u32,
    /// The signed extensions' explicit bytes.
    pub included_in_extrinsic: Vec<u8>,
    /// The signed extensions' implicit bytes.
    pub included_in_signed_data: Vec<u8>,
}

fn invalid_request() -> ClientError {
    ClientError::Protocol("invalid versioned post-quantum signing request".into())
}

fn era_value(era: &serde_json::Value) -> Result<rtao_core::codec::Value, ClientError> {
    use rtao_core::codec::Value;
    match era {
        serde_json::Value::String(text) if text == "00" => Ok(Value::Str("00".into())),
        serde_json::Value::Object(fields) => {
            let field = |name: &str| fields.get(name).and_then(serde_json::Value::as_u64);
            match (field("period"), field("current")) {
                (Some(period), Some(current)) if fields.len() == 2 => Ok(Value::record(vec![
                    ("period".into(), Value::Uint(period.into())),
                    ("current".into(), Value::Uint(current.into())),
                ])),
                _ => Err(invalid_request()),
            }
        }
        _ => Err(invalid_request()),
    }
}

fn hex_bytes(value: &serde_json::Value) -> Result<Vec<u8>, ClientError> {
    raw_bytes(value).ok_or_else(invalid_request)
}

fn hash_of(value: &serde_json::Value) -> Result<H256, ClientError> {
    hex_bytes(value)?
        .try_into()
        .map(H256)
        .map_err(|_| invalid_request())
}

impl PreparedPqExtrinsic {
    /// The signing account.
    pub fn account(&self) -> AccountId {
        self.authority.account
    }
    /// The complete native payload: call, explicit and implicit extension bytes.
    pub fn payload(&self) -> Vec<u8> {
        let mut payload = self.call_data.clone();
        payload.extend_from_slice(&self.included_in_extrinsic);
        payload.extend_from_slice(&self.included_in_signed_data);
        payload
    }
    /// The request format for its scheme.
    pub fn format(&self) -> &'static str {
        if self
            .authority
            .authority_type
            .profile()
            .is_some_and(|p| (6..=9).contains(&p))
        {
            PQ_REQUEST_FORMAT
        } else {
            MLDSA_REQUEST_FORMAT
        }
    }
    /// Python `validate`: internal consistency, without a chain read.
    pub fn validate(&self) -> Result<(), ClientError> {
        let authority = &self.authority;
        let kind = authority.authority_type;
        let derived = rtao_primitives::receiving::descriptor_account(&authority.descriptor)
            .map_err(|_| invalid_request())?;
        let stateless = is_stateless_pq_descriptor(&authority.descriptor);
        if !kind.is_post_quantum()
            || AuthorityType::from_descriptor(&authority.descriptor) != Some(kind)
            || derived != authority.account
            || (stateless
                && (authority.generation != 0
                    || authority.commitment != authority.descriptor[2..]
                    || authority.nonce >= u64::from(u32::MAX)))
            || authority.generation == u64::MAX
            || authority.commitment.len() != 64
            || authority.commitment.iter().all(|byte| *byte == 0)
            || self.payload().len() > MAX_NATIVE_PAYLOAD
        {
            return Err(invalid_request());
        }
        era_value(&self.era)?;
        Ok(())
    }
    /// Python `to_dict`: the request as JSON (`format`, `unsigned`, `authority`).
    pub fn to_json(&self) -> Result<serde_json::Value, ClientError> {
        self.validate()?;
        let hex = |bytes: &[u8]| format!("0x{}", hex::encode(bytes));
        let authority = &self.authority;
        Ok(serde_json::json!({
            "format": self.format(),
            "unsigned": {
                "call_data": hex(&self.call_data),
                "address": authority.account.to_string(),
                "public_key": hex(&authority.account.0),
                "crypto_type": authority.authority_type.code(),
                "era": self.era,
                "nonce": authority.nonce,
                "tip": self.tip,
                "tip_asset_id": self.tip_asset_id,
                "genesis_hash": hex(&self.genesis_hash.0),
                "era_block_hash": hex(&self.era_block_hash.0),
                "spec_version": self.spec_version,
                "transaction_version": self.transaction_version,
                "metadata_hash": null,
                "payload": hex(&self.payload()),
                "payload_json": {"format": self.format()},
                "included_in_extrinsic": hex(&self.included_in_extrinsic),
                "included_in_signed_data": hex(&self.included_in_signed_data),
            },
            "authority": {
                "block_hash": hex(&authority.block_hash.0),
                "address": authority.account.to_string(),
                "crypto_type": authority.authority_type.code(),
                "descriptor": hex(&authority.descriptor),
                "generation": authority.generation,
                "commitment": hex(&authority.commitment),
                "nonce": authority.nonce,
            },
        }))
    }
    /// Python `from_dict`: parse and validate a request, refusing any other
    /// format, field set or inconsistency.
    pub fn from_json(value: &serde_json::Value) -> Result<Self, ClientError> {
        let object = value.as_object().ok_or_else(invalid_request)?;
        let keys: std::collections::BTreeSet<&str> = object.keys().map(String::as_str).collect();
        if keys != ["authority", "format", "unsigned"].into_iter().collect() {
            return Err(ClientError::Protocol(
                "unsupported post-quantum signing request format".into(),
            ));
        }
        let format = object["format"].as_str().unwrap_or_default();
        if format != MLDSA_REQUEST_FORMAT && format != PQ_REQUEST_FORMAT {
            return Err(ClientError::Protocol(
                "unsupported post-quantum signing request format".into(),
            ));
        }
        let unsigned = object["unsigned"].as_object().ok_or_else(invalid_request)?;
        let authority = object["authority"]
            .as_object()
            .ok_or_else(invalid_request)?;
        let u64_of = |map: &serde_json::Map<String, serde_json::Value>, name: &str| {
            map.get(name)
                .and_then(serde_json::Value::as_u64)
                .ok_or_else(invalid_request)
        };
        let kind = u8::try_from(u64_of(authority, "crypto_type")?)
            .ok()
            .and_then(|code| AuthorityType::try_from(code).ok())
            .ok_or_else(invalid_request)?;
        let account: AccountId = authority
            .get("address")
            .and_then(serde_json::Value::as_str)
            .and_then(|address| address.parse().ok())
            .ok_or_else(invalid_request)?;
        let snapshot = HashedSigningSnapshot {
            block_hash: hash_of(&authority["block_hash"])?,
            account,
            authority_type: kind,
            descriptor: hex_bytes(&authority["descriptor"])?,
            generation: u64_of(authority, "generation")?,
            commitment: hex_bytes(&authority["commitment"])?,
            nonce: u64_of(authority, "nonce")?,
        };
        let prepared = Self {
            call_data: hex_bytes(&unsigned["call_data"])?,
            authority: snapshot,
            era: unsigned.get("era").cloned().ok_or_else(invalid_request)?,
            tip: u64_of(unsigned, "tip")?,
            tip_asset_id: match unsigned.get("tip_asset_id") {
                Some(serde_json::Value::Null) => None,
                Some(value) => Some(value.as_u64().ok_or_else(invalid_request)?),
                None => return Err(invalid_request()),
            },
            genesis_hash: hash_of(&unsigned["genesis_hash"])?,
            era_block_hash: hash_of(&unsigned["era_block_hash"])?,
            spec_version: u32::try_from(u64_of(unsigned, "spec_version")?)
                .map_err(|_| invalid_request())?,
            transaction_version: u32::try_from(u64_of(unsigned, "transaction_version")?)
                .map_err(|_| invalid_request())?,
            included_in_extrinsic: hex_bytes(&unsigned["included_in_extrinsic"])?,
            included_in_signed_data: hex_bytes(&unsigned["included_in_signed_data"])?,
        };
        prepared.validate()?;
        let account_fields = unsigned
            .get("address")
            .and_then(serde_json::Value::as_str)
            .and_then(|address| address.parse::<AccountId>().ok())
            == Some(account)
            && hex_bytes(&unsigned["public_key"])? == account.0
            && u64_of(unsigned, "crypto_type")? == u64::from(kind.code())
            && u64_of(unsigned, "nonce")? == prepared.authority.nonce
            && unsigned
                .get("metadata_hash")
                .is_some_and(serde_json::Value::is_null)
            && hex_bytes(&unsigned["payload"])? == prepared.payload();
        if !account_fields {
            return Err(invalid_request());
        }
        if format != prepared.format() {
            return Err(ClientError::Protocol(
                "signing request format does not match its cryptographic profile".into(),
            ));
        }
        Ok(prepared)
    }
    fn params(&self) -> Result<TxParams, ClientError> {
        Ok(TxParams {
            era: era_value(&self.era)?,
            nonce: self.authority.nonce,
            tip: self.tip.into(),
            tip_asset_id: self.tip_asset_id.map(u128::from),
            genesis_hash: self.genesis_hash.0,
            era_block_hash: self.era_block_hash.0,
            metadata_hash: None,
        })
    }
    /// Python `_validate_codec_request`: rebuild the extension bytes from
    /// the reviewed fields and trusted metadata; they must be unchanged.
    fn check_codec(&self, runtime: &RuntimeHandle) -> Result<(), ClientError> {
        self.validate()?;
        if runtime.spec_version() != self.spec_version
            || runtime.transaction_version() != self.transaction_version
        {
            return Err(ClientError::Protocol(
                "runtime changed since post-quantum preparation; prepare and sign again".into(),
            ));
        }
        CallBytes::decode(&self.call_data, runtime).map_err(ClientError::from)?;
        check_scheme(runtime, self.authority.authority_type)?;
        let (extra, additional) = runtime
            .core()
            .signature_payload_parts(&self.params()?)
            .map_err(CoreError::from)?;
        if extra != self.included_in_extrinsic || additional != self.included_in_signed_data {
            return Err(ClientError::Protocol(
                "post-quantum prepared call, extensions or implicit values changed".into(),
            ));
        }
        Ok(())
    }
}

/// Python `sign_prepared_mldsa` (`sign_prepared_pq`): sign a reviewed
/// request offline, against independently trusted metadata (`runtime`) and
/// chain identity (`expected_genesis`).
///
/// The extension bytes are rebuilt from the displayed fields before the key
/// is used; the key must be the request's descriptor and, for a rotating
/// account, match the requested generation's commitment.
#[doc(alias = "bittensor.mldsa_signing.sign_prepared_mldsa")]
#[doc(alias = "bittensor.mldsa_signing.sign_prepared_pq")]
pub async fn sign_prepared_pq(
    prepared: &PreparedPqExtrinsic,
    signer: &dyn AuthoritySigner,
    runtime: &RuntimeHandle,
    expected_genesis: H256,
) -> Result<rtao_signer::AuthorizationProof, ClientError> {
    prepared.validate()?;
    if expected_genesis != prepared.genesis_hash {
        return Err(ClientError::Protocol(
            "post-quantum request does not match the trusted signing chain".into(),
        ));
    }
    prepared.check_codec(runtime)?;
    let authority = &prepared.authority;
    if !signer.authority_type().is_post_quantum() || signer.descriptor() != authority.descriptor {
        return Err(ClientError::Protocol(
            "post-quantum signing request belongs to another wallet".into(),
        ));
    }
    if signer
        .commitment(authority.generation)
        .map_err(ClientError::Signing)?
        != authority.commitment
    {
        return Err(ClientError::Protocol(
            "offline key does not match the requested authority commitment".into(),
        ));
    }
    let payload = prepared.payload();
    let request = AuthorizationRequest {
        payload: &payload,
        call: &prepared.call_data,
        generation: authority.generation,
        purpose: SignPurpose::Extrinsic,
    };
    let proof = signer
        .sign_authorization(&request)
        .await
        .map_err(ClientError::Signing)?;
    proof
        .check(signer.authority_type())
        .map_err(ClientError::Signing)?;
    Ok(proof)
}

impl Client {
    /// Python `Client.prepare_mldsa_call` / `prepare_pq_call`: prepare a
    /// versioned full-payload request for an offline post-quantum signer
    /// identified by its public `descriptor`.
    ///
    /// A rotating account's finalized authority must have no pending
    /// transaction; a standard account's request takes the node's next index
    /// (never the pipelining cache: the proof may return much later).
    #[doc(alias = "bittensor.client.Client.prepare_pq_call")]
    #[doc(alias = "bittensor.client.Client.prepare_mldsa_call")]
    pub async fn prepare_pq_call(
        &self,
        call: CallBytes,
        descriptor: &[u8],
        opts: &SubmitOptions,
    ) -> Result<PreparedPqExtrinsic, ClientError> {
        let kind = AuthorityType::from_descriptor(descriptor)
            .filter(|kind| kind.is_post_quantum())
            .ok_or_else(|| {
                ClientError::Protocol(
                    "offline post-quantum preparation requires a post-quantum descriptor".into(),
                )
            })?;
        let public = PublicAuthority {
            account: rtao_primitives::receiving::descriptor_account(descriptor)?,
            kind,
            descriptor: descriptor.to_vec(),
        };
        let snapshot = if kind.is_stateless() {
            let mut snapshot = self.hashed_snapshot(&public, BlockRef::Finalized).await?;
            snapshot.nonce = self.nonces().fresh(&public.account).await?;
            snapshot
        } else {
            self.prepare_hashed_snapshot(&public).await?
        };
        self.ensure_signing_network()?;
        let (runtime, version) = self.refresh_runtime_version().await?;
        call.ensure_runtime(&runtime).map_err(ClientError::from)?;
        check_live_stamp(&version, &call)?;
        check_scheme(&runtime, kind)?;
        let height = self.finalized_number().await?;
        let (era, anchor, _) = self.era_for(opts.mortality, height).await?;
        let era = match &era {
            rtao_core::codec::Value::Str(text) => serde_json::Value::String(text.clone()),
            _ => match opts.mortality {
                super::Mortality::Mortal(period) => {
                    serde_json::json!({"period": period.get(), "current": height})
                }
                super::Mortality::Immortal => serde_json::Value::String("00".into()),
            },
        };
        let mut prepared = PreparedPqExtrinsic {
            call_data: call.as_bytes().to_vec(),
            authority: snapshot,
            era,
            tip: 0,
            tip_asset_id: None,
            genesis_hash: self.genesis(),
            era_block_hash: anchor,
            spec_version: runtime.spec_version(),
            transaction_version: runtime.transaction_version(),
            included_in_extrinsic: Vec::new(),
            included_in_signed_data: Vec::new(),
        };
        let (extra, additional) = runtime
            .core()
            .signature_payload_parts(&prepared.params()?)
            .map_err(CoreError::from)?;
        prepared.included_in_extrinsic = extra;
        prepared.included_in_signed_data = additional;
        prepared.validate()?;
        Ok(prepared)
    }

    /// Python `submit_signature` for a post-quantum request: recheck the
    /// account's current authority and the runtime, verify `proof` against
    /// the request's authority, then submit the signed extrinsic.
    ///
    /// A rotating account's finalized authority and nonce must be unchanged
    /// since preparation; a standard account's nonce must not have been
    /// consumed.
    pub async fn submit_pq_proof(
        &self,
        prepared: &PreparedPqExtrinsic,
        proof: &[u8],
        opts: &SubmitOptions,
    ) -> Result<ExtrinsicReport, SubmitFailure> {
        prepared.validate()?;
        let authority = &prepared.authority;
        let public = PublicAuthority {
            account: authority.account,
            kind: authority.authority_type,
            descriptor: authority.descriptor.clone(),
        };
        let current = if authority.authority_type.is_stateless() {
            let current = self.hashed_snapshot(&public, BlockRef::Finalized).await?;
            if authority.nonce < current.nonce {
                return Err(
                    refused("post-quantum transaction nonce has already been consumed").into(),
                );
            }
            HashedSigningSnapshot {
                nonce: authority.nonce,
                ..current
            }
        } else {
            self.prepare_hashed_snapshot(&public).await?
        };
        if (
            &current.descriptor,
            current.generation,
            &current.commitment,
            current.nonce,
        ) != (
            &authority.descriptor,
            authority.generation,
            &authority.commitment,
            authority.nonce,
        ) || self.genesis() != prepared.genesis_hash
        {
            return Err(refused(
                "post-quantum authority or chain changed since preparation; prepare and sign again",
            )
            .into());
        }
        let runtime = self.refresh_runtime().await?;
        prepared.check_codec(&runtime)?;
        let payload = prepared.payload();
        verify_proof(
            authority.authority_type,
            proof,
            &payload,
            &authority.descriptor,
            authority.generation,
            &authority.commitment,
        )
        .map_err(|_| ClientError::Protocol("invalid post-quantum transaction proof".into()))?;
        let (bytes, hash) = runtime
            .core()
            .encode_signed_extrinsic(
                &prepared.call_data,
                authority.account.0,
                proof,
                authority.authority_type.code(),
                &prepared.params()?,
            )
            .map_err(CoreError::from)?;
        let hash = H256(hash);
        let limits = SubmissionLimits::new(opts)?;
        let attempted_hash = SubmissionAttempt::default();
        let lease = self
            .nonces()
            .lease_explicit(&authority.account, authority.nonce);
        let expiry = match &prepared.era {
            serde_json::Value::Object(fields) => {
                let period = fields.get("period").and_then(serde_json::Value::as_u64);
                let current = fields.get("current").and_then(serde_json::Value::as_u64);
                period.zip(current).and_then(|(period, current)| {
                    rtao_core::extrinsic::era_birth(period, current).checked_add(period)
                })
            }
            _ => None,
        };
        let head = self.finalized_number().await?;
        let operation = async {
            match self
                .send_and_watch(&bytes, hash, opts, limits.admission, &attempted_hash, None)
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
                        submission_attempt_error(self, authority.account, error, &attempted_hash);
                    if (opts.reconcile && unknown_fate(&error))
                        || attempted_hash.retracted.load(Ordering::Acquire)
                    {
                        limits.full.check(self)?;
                        self.reconcile(hash, head, expiry).await
                    } else {
                        Err(error)
                    }
                }
            }
        };
        limits.full.run(self, operation).await.map_err(|error| {
            submission_attempt_error(self, authority.account, error, &attempted_hash)
        })
    }
}
