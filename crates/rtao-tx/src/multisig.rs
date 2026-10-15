// Derived from RaoFoundation/subtensor sdk/python/bittensor/multisig.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/_transport/contract.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Multisig account approvals and best-effort funding checks.

use crate::{ChainError, ExecuteOptions, Executor, ExtrinsicResult, IntentError, TxError};
use rtao_catalog::py::RAO_PER_TAO;
use rtao_client::{CallBytes, Client, ClientError, View, ViewExt};
use rtao_core::{CoreError, DynValue, value_ext::ValueExt};
use rtao_primitives::{AccountId, Balance, CryptoType, Tao};
use rtao_signer::{DynKeySource, KeyView, SignerRole};
use std::time::Duration;

/// One M-of-N signer set and the core-derived dispatch account.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Multisig {
    /// Signatories in canonical account-byte order.
    pub signatories: Vec<AccountId>,
    /// Required approval count.
    pub threshold: u16,
    /// Account derived by the core from the signer set and threshold.
    pub address: AccountId,
}

/// Public signer selection and waiting options for one approval.
#[derive(Clone, Debug)]
pub struct ApproveOptions {
    /// Wallet role to resolve, normally the coldkey.
    pub signer: SignerRole,
    /// Resolve the approval's inclusion when finality is not requested.
    pub wait_for_inclusion: bool,
    /// Await finalized inclusion, including the shield's inner approval.
    pub wait_for_finalization: bool,
    /// Encrypt the complete multisig approval with an eight-block era.
    pub shielded: bool,
    /// Bound approval preparation, signing and waiting together.
    pub deadline: Option<Duration>,
    /// Bound signing and author acknowledgment separately from waiting.
    pub submit_deadline: Option<Duration>,
    /// Optional display label in funding remediation.
    pub signer_label: Option<String>,
}
impl Default for ApproveOptions {
    fn default() -> Self {
        Self {
            signer: SignerRole::Coldkey,
            wait_for_inclusion: true,
            wait_for_finalization: false,
            shielded: false,
            deadline: None,
            submit_deadline: None,
            signer_label: None,
        }
    }
}

impl Multisig {
    /// Derive an account after validating the set against the pinned runtime.
    pub fn new(
        runtime: &rtao_client::RuntimeHandle,
        signatories: &[AccountId],
        threshold: u16,
    ) -> Result<Self, TxError> {
        if signatories.len() < 2 {
            return Err(IntentError::InvalidArgument(
                "multisig requires at least two signatories".into(),
            )
            .into());
        }
        if threshold == 0 || usize::from(threshold) > signatories.len() {
            return Err(IntentError::InvalidArgument(
                "multisig threshold is outside the signer set".into(),
            )
            .into());
        }
        rtao_client::compose::validate_multisig_signatory_count(runtime, signatories.len())
            .map_err(|error| IntentError::Validation(error.to_string()))?;
        let public_keys = signatories
            .iter()
            .map(|account| account.0)
            .collect::<Vec<_>>();
        let (address, sorted) = rtao_core::extrinsic::multisig_account_id(&public_keys, threshold)
            .map_err(CoreError::from)
            .map_err(IntentError::from)?;
        if sorted.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(IntentError::InvalidArgument("duplicate multisig signatory".into()).into());
        }
        Ok(Self {
            signatories: sorted.into_iter().map(AccountId).collect(),
            threshold,
            address: AccountId(address),
        })
    }

    /// Approve the exact call, preserving an existing opening timepoint.
    ///
    /// Chain-read and fee-probe failures skip the funding check. An observed
    /// shortfall refuses before unlocking or signing. The submitted wrapper
    /// always contains the full call, so a threshold-reaching approval executes.
    pub async fn approve(
        &self,
        executor: &Executor,
        call: CallBytes,
        keys: &dyn DynKeySource,
        options: &ApproveOptions,
    ) -> Result<ExtrinsicResult, TxError> {
        let signer = public_signer(keys, options.signer)?;
        if !self.signatories.contains(&signer) {
            return Err(
                IntentError::InvalidArgument("signer is not a multisig signatory".into()).into(),
            );
        }
        let started = tokio::time::Instant::now();
        let expires = options
            .deadline
            .map(|limit| {
                started.checked_add(limit).ok_or_else(|| {
                    IntentError::InvalidArgument("multisig approval deadline overflows time".into())
                })
            })
            .transpose()?;
        let preparation = async {
            let client = executor.client();
            let snapshot = View::pin(client).await?;
            let canonical =
                Self::new(&snapshot.block().runtime, &self.signatories, self.threshold)?;
            if canonical.address != self.address {
                return Err(IntentError::InvalidArgument(
                    "multisig address does not match its signer set".into(),
                )
                .into());
            }
            call.ensure_runtime(&snapshot.block().runtime)
                .map_err(ClientError::from)?;
            self.preflight_funds(
                client,
                &snapshot,
                signer,
                &call,
                options.signer_label.as_deref(),
            )
            .await?;
            let runtime = &snapshot.block().runtime;
            let outer = if self.threshold == 1 {
                let others = self
                    .signatories
                    .iter()
                    .copied()
                    .filter(|account| *account != signer)
                    .collect::<Vec<_>>();
                rtao_client::compose::as_multi_threshold_1(runtime, &others, call)
                    .map_err(ClientError::from)?
            } else {
                let estimate = client
                    .estimate_weight(call.clone(), signer, CryptoType::Sr25519)
                    .await?;
                rtao_client::compose::as_multi(
                    &snapshot,
                    signer,
                    &self.signatories,
                    self.threshold,
                    call,
                    &estimate,
                )
                .await?
            };
            Ok::<_, TxError>(outer)
        };
        let outer = match expires {
            Some(expires) => tokio::time::timeout_at(expires, preparation)
                .await
                .map_err(|_| approval_timeout())??,
            None => preparation.await?,
        };
        let remaining = match expires {
            Some(expires) => Some(
                expires
                    .checked_duration_since(tokio::time::Instant::now())
                    .ok_or_else(approval_timeout)?,
            ),
            None => None,
        };
        executor
            .submit_call_dyn(
                outer,
                keys,
                options.signer,
                &ExecuteOptions {
                    period: if options.shielded {
                        Some(rtao_catalog::py::MEV_SHIELD_ERA_PERIOD as u32)
                    } else {
                        Some(rtao_catalog::py::DEFAULT_ERA_PERIOD as u32)
                    },
                    wait_for_inclusion: options.wait_for_inclusion,
                    wait_for_finalization: options.wait_for_finalization,
                    shielded: options.shielded,
                    deadline: remaining,
                    submit_deadline: options.submit_deadline,
                    ..Default::default()
                },
            )
            .await
    }

    async fn preflight_funds<V: View + ?Sized>(
        &self,
        client: &Client,
        view: &V,
        signer: AccountId,
        call: &CallBytes,
        label: Option<&str>,
    ) -> Result<(), TxError> {
        let preparation = async {
            let pending = view
                .storage(
                    &rtao_runtime::storage::multisig::MULTISIGS,
                    (self.address, call.hash()),
                )
                .await?;
            let outer = if self.threshold >= 2 {
                let runtime = view.runtime().await?;
                let estimate = client
                    .estimate_weight(call.clone(), signer, CryptoType::Sr25519)
                    .await?;
                let others = self
                    .signatories
                    .iter()
                    .copied()
                    .filter(|account| *account != signer)
                    .collect::<Vec<_>>();
                Some(
                    rtao_runtime::calls::multisig::as_multi(
                        self.threshold,
                        others,
                        None,
                        call.clone(),
                        DynValue::new(estimate.weight),
                    )
                    .compose(&runtime)
                    .map_err(ClientError::from)?,
                )
            } else {
                None
            };
            Ok::<_, ClientError>((pending.is_none(), outer))
        }
        .await;
        let Ok((opening, outer)) = preparation else {
            return Ok(());
        };
        check_multisig_funds(
            client,
            view,
            FundsRequest {
                signer,
                threshold: self.threshold,
                opening,
                outer_call: outer.as_ref(),
                crypto_type: CryptoType::Sr25519,
                signer_label: label,
            },
        )
        .await
    }
}

fn approval_timeout() -> TxError {
    TxError::Rpc(rtao_rpc::RpcError::Timeout {
        detail: "multisig approval timed out without reaching the requested state".into(),
    })
}

impl Executor {
    /// Derive an account only after checking the pinned runtime's signer limit.
    pub async fn multisig(
        &self,
        signatories: &[AccountId],
        threshold: u16,
    ) -> Result<Multisig, TxError> {
        let snapshot = View::pin(self.client()).await?;
        Multisig::new(&snapshot.block().runtime, signatories, threshold)
    }
}

fn public_signer(keys: &dyn KeyView, role: SignerRole) -> Result<AccountId, TxError> {
    if let Some(account) = keys.bare_signer_account() {
        return Ok(account);
    }
    Ok(match role {
        SignerRole::Coldkey => keys.coldkey_address()?,
        SignerRole::Hotkey => keys.hotkey_address()?,
    })
}

/// Public inputs to the reusable multisig funds check.
pub struct FundsRequest<'a> {
    /// Signatory paying the opening deposit and approval fee.
    pub signer: AccountId,
    /// Required approval count.
    pub threshold: u16,
    /// Whether the call hash has no pending operation.
    pub opening: bool,
    /// Exact outer approval to fee-probe, if one is available.
    pub outer_call: Option<&'a CallBytes>,
    /// Public signature algorithm for the zero-signature fee probe.
    pub crypto_type: CryptoType,
    /// Local display label for the funding help.
    pub signer_label: Option<&'a str>,
}

/// Read the opening reserve through the view's generated constant descriptors.
pub async fn multisig_deposit<V: View + ?Sized>(
    view: &V,
    threshold: u16,
) -> Result<Balance<Tao>, ClientError> {
    let (base, factor) = tokio::try_join!(
        view.constant(&rtao_runtime::constants::multisig::DEPOSIT_BASE),
        view.constant(&rtao_runtime::constants::multisig::DEPOSIT_FACTOR),
    )?;
    let reserve = factor
        .0
        .checked_mul(u64::from(threshold))
        .and_then(|value| base.0.checked_add(value))
        .ok_or_else(|| ClientError::Protocol("multisig deposit exceeds native u64 rao".into()))?;
    Ok(Balance::from_rao(reserve))
}

/// Read a signatory's free TAO through `System.Account` at the given view.
pub async fn free_balance<V: View + ?Sized>(
    view: &V,
    signer: AccountId,
) -> Result<Balance<Tao>, ClientError> {
    let account = view
        .storage(&rtao_runtime::storage::system::ACCOUNT, (signer,))
        .await?;
    let free = account
        .as_core()
        .field("data")
        .and_then(|data| data.field("free"))
        .and_then(ValueExt::as_u128)
        .and_then(|free| u64::try_from(free).ok())
        .ok_or_else(|| {
            ClientError::Protocol("System.Account omitted native u64 free balance".into())
        })?;
    Ok(Balance::from_rao(free))
}

/// Refuse an observed deposit-plus-fee shortfall; skip failed state or fee probes.
pub async fn check_multisig_funds<V: View + ?Sized>(
    client: &Client,
    view: &V,
    request: FundsRequest<'_>,
) -> Result<(), TxError> {
    let observed = async {
        let deposit = if request.opening && request.threshold >= 2 {
            multisig_deposit(view, request.threshold).await?.rao()
        } else {
            0
        };
        let fee = match request.outer_call {
            Some(call) => {
                client
                    .estimate_weight(call.clone(), request.signer, request.crypto_type)
                    .await?
                    .partial_fee
            }
            None => 0,
        };
        if deposit == 0 && fee == 0 {
            return Ok(None);
        }
        let free = free_balance(view, request.signer).await?.rao();
        Ok::<_, ClientError>(Some((free, fee, deposit)))
    }
    .await;
    let Ok(Some((free, fee, deposit))) = observed else {
        return Ok(());
    };
    if u128::from(free) >= u128::from(deposit) + u128::from(fee) {
        return Ok(());
    }
    Err(TxError::Chain(funding_error(
        request.signer,
        request.signer_label,
        free,
        fee,
        deposit,
    )))
}

/// Return Python's opening-deposit warning when its best-effort probe finds a shortfall.
pub async fn multisig_opening_shortfall<V: View + ?Sized>(
    view: &V,
    signer: AccountId,
    threshold: u16,
) -> Option<String> {
    if threshold < 2 {
        return None;
    }
    let deposit = multisig_deposit(view, threshold).await.ok()?.rao();
    let free = free_balance(view, signer).await.ok()?.rao();
    if free >= deposit {
        return None;
    }
    let amount = tao_ceil(
        u128::from(deposit) + u128::from(rtao_catalog::py::MULTISIG_FEE_ALLOWANCE_RAO)
            - u128::from(free),
    );
    Some(format!(
        "the opening approval reserves a {} multisig deposit the signer cannot cover: fund {signer} with ≥ {amount} — the deposit is returned when the op executes",
        tao(u128::from(deposit))
    ))
}

fn funding_error(
    signer: AccountId,
    label: Option<&str>,
    free: u64,
    fee: u64,
    deposit: u64,
) -> ChainError {
    let display = match label.filter(|label| !label.is_empty() && *label != signer.to_string()) {
        Some(label) => format!("{signer} ({label})"),
        None => signer.to_string(),
    };
    let amount =
        tao_ceil((u128::from(deposit) + 2 * u128::from(fee)).saturating_sub(u128::from(free)));
    let (message, help) = if deposit > 0 {
        (
            format!(
                "the signing account cannot cover the multisig deposit plus the transaction fee: {signer} holds {} free, but opening this operation reserves a {} deposit on top of a ~{} fee",
                tao(u128::from(free)),
                tao(u128::from(deposit)),
                tao(u128::from(fee))
            ),
            format!(
                "fund {display} with ≥ {amount} — the deposit is returned when the op executes"
            ),
        )
    } else {
        (
            format!(
                "the signing account cannot cover the transaction fee: {signer} holds {} free, but this approval costs a ~{} fee",
                tao(u128::from(free)),
                tao(u128::from(fee))
            ),
            format!("fund {display} with ≥ {amount}"),
        )
    };
    ChainError::with_code(message, None, rtao_catalog::ErrorCode::InsufficientBalance)
        .with_remediation(help)
}

fn tao(rao: u128) -> String {
    let unit = u128::from(RAO_PER_TAO);
    let text = format!("{}.{:09}", rao / unit, rao % unit);
    format!("τ{}", text.trim_end_matches('0').trim_end_matches('.'))
}

fn tao_ceil(rao: u128) -> String {
    let units = rao.div_ceil(u128::from(RAO_PER_TAO / 1000));
    let text = format!("{}.{:03}", units / 1000, units % 1000);
    format!("τ{}", text.trim_end_matches('0').trim_end_matches('.'))
}
