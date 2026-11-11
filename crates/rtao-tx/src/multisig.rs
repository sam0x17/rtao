// Derived from RaoFoundation/subtensor sdk/python/bittensor/multisig.py @ b6a81d144078eac18bd25c3af7cb31579bb670f3 (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/_transport/contract.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Multisig account approvals and best-effort funding checks.

use crate::{ChainError, ExecuteOptions, Executor, ExtrinsicResult, IntentError, TxError};
use rtao_catalog::py::RAO_PER_TAO;
use rtao_client::{CallBytes, Client, ClientError, View, ViewExt};
use rtao_core::{DynValue, value_ext::ValueExt};
use rtao_primitives::{AccountId, AccountMode, Balance, CryptoType, Tao};
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
    /// The derivation of [`Self::address`]; every approval uses its calls.
    pub account_mode: AccountMode,
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
    /// Derive the legacy account after validating the set against the pinned runtime.
    pub fn new(
        runtime: &rtao_client::RuntimeHandle,
        signatories: &[AccountId],
        threshold: u16,
    ) -> Result<Self, TxError> {
        Self::with_mode(runtime, signatories, threshold, AccountMode::Legacy)
    }

    /// Derive the account of `mode` after validating the set against the
    /// pinned runtime. A `Safe` group's account cannot be a raw classical key
    /// and is a different account from the same members' legacy one.
    pub fn with_mode(
        runtime: &rtao_client::RuntimeHandle,
        signatories: &[AccountId],
        threshold: u16,
        account_mode: AccountMode,
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
        let mut unique = signatories.to_vec();
        unique.sort_unstable();
        if unique.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(IntentError::InvalidArgument("duplicate multisig signatory".into()).into());
        }
        let (address, sorted) =
            rtao_primitives::multisig_account_id(signatories, threshold, account_mode)
                .map_err(IntentError::from)?;
        Ok(Self {
            signatories: sorted,
            threshold,
            address,
            account_mode,
        })
    }

    /// Python `Multisig.protection_status`: finalized eligibility for, and
    /// quorum-approved registration of, this group's permanent derived
    /// authority. A safe group needs none.
    #[doc(alias = "bittensor.multisig.Multisig.protection_status")]
    pub async fn protection_status(
        &self,
        client: &Client,
    ) -> Result<crate::hashed::MultisigProtection, TxError> {
        if self.account_mode == AccountMode::Safe {
            return Ok(crate::hashed::MultisigProtection {
                safe_derivation: true,
                ..Default::default()
            });
        }
        let finalized = client.at(rtao_client::BlockRef::Finalized).await?;
        let runtime = finalized.block().runtime.clone();
        Ok(crate::hashed::multisig_protection_status(
            &finalized,
            &runtime,
            &self.signatories,
            self.threshold,
        )
        .await?)
    }

    /// Python `Multisig.protection_call`: the permanent authority
    /// registration (or, once registered, its check) for this group's own
    /// quorum to approve before funding. `deposit_payer` must be the final
    /// executing member, consenting through its outer signature. Approve it
    /// separately so pending economic calls keep their bytes.
    #[doc(alias = "bittensor.multisig.Multisig.protection_call")]
    pub async fn protection_call(
        &self,
        client: &Client,
        deposit_payer: AccountId,
    ) -> Result<CallBytes, TxError> {
        if self.account_mode == AccountMode::Safe {
            return Err(IntentError::validation(
                "safe multisigs do not use permanent authority registration",
            )
            .into());
        }
        let finalized = client.at(rtao_client::BlockRef::Finalized).await?;
        let runtime = finalized.block().runtime.clone();
        Ok(crate::hashed::multisig_protection_call(
            &finalized,
            &runtime,
            &self.signatories,
            self.threshold,
            deposit_payer,
        )
        .await?)
    }

    /// Python `Multisig.funding_call`: `Balances.transfer_keep_alive` to the
    /// group, guarded by `check_derived` in one atomic batch when the group
    /// is eligible for protection. An eligible group that has not registered
    /// its authority (through [`Self::protection_call`], finalized) is
    /// refused.
    #[doc(alias = "bittensor.multisig.Multisig.funding_call")]
    pub async fn funding_call(
        &self,
        client: &Client,
        amount_rao: u64,
    ) -> Result<CallBytes, TxError> {
        if amount_rao == 0 {
            return Err(IntentError::validation(
                "funding amount must be a positive integer number of rao",
            )
            .into());
        }
        let status = self.protection_status(client).await?;
        let runtime = client
            .at(rtao_client::BlockRef::Finalized)
            .await?
            .block()
            .runtime
            .clone();
        let payment = rtao_runtime::calls::balances::transfer_keep_alive(
            DynValue::new(rtao_core::codec::Value::Bytes(self.address.0.to_vec())),
            rtao_primitives::TaoBalance(amount_rao),
        )
        .compose(&runtime)
        .map_err(IntentError::from)?;
        if !status.eligible {
            return Ok(payment);
        }
        if !status.registered {
            return Err(IntentError::validation(
                "this multisig must approve protection_call(deposit_payer=...) through its quorum and finalize it before funding",
            )
            .into());
        }
        // Registration is already final, so this is `check_derived` only.
        let guard = self.protection_call(client, self.address).await?;
        Ok(
            rtao_runtime::calls::utility::batch_all(vec![guard, payment])
                .compose(&runtime)
                .map_err(IntentError::from)?,
        )
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
            let canonical = Self::with_mode(
                &snapshot.block().runtime,
                &self.signatories,
                self.threshold,
                self.account_mode,
            )?;
            if canonical.address != self.address {
                return Err(IntentError::InvalidArgument(
                    "multisig address does not match its signer set".into(),
                )
                .into());
            }
            call.ensure_runtime(&snapshot.block().runtime)
                .map_err(ClientError::from)?;
            // Python v488: account registration defaults to Shield when it is
            // active, unless the executor opted out (D-170).
            let shielded = options.shielded
                || executor
                    .auto_shield_route(&snapshot, &snapshot.block().runtime, &call, &mut Vec::new())
                    .await?;
            // A hashed or post-quantum signer's fee probes price its proof.
            let descriptor = keys
                .authority_descriptor(rtao_signer::SignerRole::Coldkey)
                .filter(|descriptor| {
                    rtao_primitives::receiving::descriptor_account(descriptor)
                        .is_ok_and(|account| account == signer)
                });
            self.preflight_funds(
                client,
                &snapshot,
                signer,
                &call,
                options.signer_label.as_deref(),
                shielded,
                descriptor.as_deref(),
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
                rtao_client::compose::as_multi_threshold_1_with_mode(
                    runtime,
                    &others,
                    call,
                    self.account_mode,
                )
                .map_err(ClientError::from)?
            } else {
                let estimate = client
                    .estimate_weight(call.clone(), signer, CryptoType::Sr25519)
                    .await?;
                rtao_client::compose::as_multi_with_mode(
                    &snapshot,
                    signer,
                    &self.signatories,
                    self.threshold,
                    call,
                    &estimate,
                    self.account_mode,
                )
                .await?
            };
            Ok::<_, TxError>((outer, shielded))
        };
        let (outer, shielded) = match expires {
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
                    period: if shielded {
                        Some(rtao_catalog::py::MEV_SHIELD_ERA_PERIOD as u32)
                    } else {
                        Some(rtao_catalog::py::DEFAULT_ERA_PERIOD as u32)
                    },
                    wait_for_inclusion: options.wait_for_inclusion,
                    wait_for_finalization: options.wait_for_finalization,
                    shielded,
                    deadline: remaining,
                    submit_deadline: options.submit_deadline,
                    ..Default::default()
                },
            )
            .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn preflight_funds<V: View + ?Sized>(
        &self,
        client: &Client,
        view: &V,
        signer: AccountId,
        call: &CallBytes,
        label: Option<&str>,
        shielded: bool,
        descriptor: Option<&[u8]>,
    ) -> Result<(), TxError> {
        let preparation = async {
            let pending = view
                .storage(
                    &rtao_runtime::storage::multisig::MULTISIGS,
                    (self.address, call.hash()),
                )
                .await?;
            let outer = if self.threshold == 1 {
                // Python v488 fee-probes the threshold-1 wrapper as well.
                let runtime = view.runtime().await?;
                let others = self
                    .signatories
                    .iter()
                    .copied()
                    .filter(|account| *account != signer)
                    .collect::<Vec<_>>();
                Some(
                    rtao_client::compose::as_multi_threshold_1_with_mode(
                        &runtime,
                        &others,
                        call.clone(),
                        self.account_mode,
                    )
                    .map_err(ClientError::from)?,
                )
            } else if self.threshold >= 2 {
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
                let weight = DynValue::new(estimate.weight);
                Some(
                    match self.account_mode {
                        AccountMode::Legacy => rtao_runtime::calls::multisig::as_multi(
                            self.threshold,
                            others,
                            None,
                            call.clone(),
                            weight,
                        ),
                        AccountMode::Safe => rtao_runtime::calls::multisig::as_multi_safe(
                            self.threshold,
                            others,
                            None,
                            call.clone(),
                            weight,
                        ),
                    }
                    .compose(&runtime)
                    .map_err(ClientError::from)?,
                )
            } else {
                None
            };
            Ok::<_, ClientError>((pending.is_none(), outer))
        }
        .await;
        let (opening, outer) = match preparation {
            Ok(prepared) => prepared,
            Err(error) if shielded => {
                return Err(TxError::Chain(ChainError::new(
                    format!("could not prepare shielded multisig approval: {error}"),
                    None,
                )));
            }
            Err(_) => return Ok(()),
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
                descriptor,
                signer_label: label,
                shielded,
            },
        )
        .await
    }
}

fn approval_timeout() -> TxError {
    TxError::Rpc(rtao_jsonrpc::RpcError::Timeout {
        detail: "multisig approval timed out without reaching the requested state".into(),
    })
}

impl Executor {
    /// Derive the legacy account only after checking the pinned runtime's signer limit.
    pub async fn multisig(
        &self,
        signatories: &[AccountId],
        threshold: u16,
    ) -> Result<Multisig, TxError> {
        self.multisig_with_mode(signatories, threshold, AccountMode::Legacy)
            .await
    }

    /// Python's `Client.multisig(signatories, threshold, account_mode=)`:
    /// derive the account of `account_mode` after checking the pinned
    /// runtime's signer limit.
    #[doc(alias = "bittensor.client.Client.multisig")]
    pub async fn multisig_with_mode(
        &self,
        signatories: &[AccountId],
        threshold: u16,
        account_mode: AccountMode,
    ) -> Result<Multisig, TxError> {
        let snapshot = View::pin(self.client()).await?;
        Multisig::with_mode(
            &snapshot.block().runtime,
            signatories,
            threshold,
            account_mode,
        )
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
    /// The signer's hashed or post-quantum public descriptor, if it is one:
    /// the fee probes then carry the scheme's proof length instead of a
    /// 64-byte signature (D-177). `None` for a classical signer.
    pub descriptor: Option<&'a [u8]>,
    /// Local display label for the funding help.
    pub signer_label: Option<&'a str>,
    /// The approval goes through MEV Shield: the check then quotes the
    /// carrier fee too, keeps the existential deposit (or a larger freeze)
    /// free, and refuses rather than skips when it cannot quote (Python
    /// v488 `check_multisig_funds(shielded=True)`).
    pub shielded: bool,
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

/// Refuse an observed deposit-plus-fee shortfall. A plain approval skips
/// failed state or fee probes; a shielded one must quote everything.
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
        let probe = match request.descriptor.and_then(|descriptor| {
            rtao_primitives::AuthorityType::from_descriptor(descriptor).zip(Some(descriptor))
        }) {
            Some((kind, descriptor)) => rtao_client::ProbeSigner::Authority { kind, descriptor },
            None => rtao_client::ProbeSigner::Classical(request.crypto_type),
        };
        let mut fee = match request.outer_call {
            Some(call) => {
                client
                    .estimate_weight_as(call.clone(), request.signer, probe)
                    .await?
                    .partial_fee
            }
            None => 0,
        };
        let (free, retained) = if request.shielded {
            let call = request
                .outer_call
                .ok_or_else(|| ClientError::Protocol("no multisig approval to quote".into()))?;
            fee = fee.saturating_add(
                client
                    .estimate_shielded_carrier_fee_as(
                        request.signer,
                        probe,
                        call.clone(),
                        rtao_catalog::py::MEV_SHIELD_ERA_PERIOD,
                    )
                    .await?
                    .rao(),
            );
            let account = view
                .storage(&rtao_runtime::storage::system::ACCOUNT, (request.signer,))
                .await?;
            let data = account.as_core().field("data");
            let field = |name: &str| {
                data.and_then(|data| data.field(name))
                    .and_then(ValueExt::as_u128)
                    .and_then(|value| u64::try_from(value).ok())
            };
            let minimum = view
                .constant(&rtao_runtime::constants::balances::EXISTENTIAL_DEPOSIT)
                .await?
                .0;
            (
                field("free").unwrap_or(0),
                minimum.max(field("frozen").unwrap_or(0)),
            )
        } else {
            if deposit == 0 && fee == 0 {
                return Ok(None);
            }
            (free_balance(view, request.signer).await?.rao(), 0)
        };
        if deposit == 0 && fee == 0 {
            return Ok(None);
        }
        Ok::<_, ClientError>(Some((free, fee, deposit, retained)))
    }
    .await;
    let (free, fee, deposit, retained) = match observed {
        Ok(Some(observed)) => observed,
        Ok(None) => return Ok(()),
        Err(error) if request.shielded => {
            return Err(TxError::Chain(ChainError::new(
                format!("could not quote complete shielded multisig approval: {error}"),
                None,
            )));
        }
        Err(_) => return Ok(()),
    };
    if u128::from(free) >= u128::from(deposit) + u128::from(fee) + u128::from(retained) {
        return Ok(());
    }
    Err(TxError::Chain(funding_error(
        request.signer,
        request.signer_label,
        free,
        fee,
        deposit,
        retained,
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
    retained: u64,
) -> ChainError {
    let display = match label.filter(|label| !label.is_empty() && *label != signer.to_string()) {
        Some(label) => format!("{signer} ({label})"),
        None => signer.to_string(),
    };
    let amount = tao_ceil(
        (u128::from(deposit) + u128::from(retained) + 2 * u128::from(fee))
            .saturating_sub(u128::from(free)),
    );
    let (mut message, help) = if deposit > 0 {
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
    if retained > 0 {
        message.push_str(&format!(
            "; {} must remain free for the existential deposit or freeze",
            tao(u128::from(retained))
        ));
    }
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

/// Merge composed-call extras without losing nested approval obligations.
///
/// Later parts overwrite earlier keys. A part that defers its inner call (an
/// approval that does not dispatch it) drops the approvals collected so far
/// and empties `pure_proxy_reserves`: the eventual permanent spend stays in the
/// metadata, but this approval does not create the pure account.
#[doc(alias = "bittensor.multisig.merge_approval_metadata")]
pub fn merge_approval_metadata(
    parts: &[&serde_json::Map<String, serde_json::Value>],
) -> serde_json::Map<String, serde_json::Value> {
    use serde_json::Value;
    let mut merged = serde_json::Map::new();
    let mut approvals = Vec::new();
    for part in parts {
        merged.extend(part.iter().map(|(key, value)| (key.clone(), value.clone())));
        if part
            .get("multisig_defers_inner")
            .is_some_and(|value| value.as_bool().unwrap_or(!value.is_null()))
        {
            approvals.clear();
            if merged.contains_key("pure_proxy_reserves") {
                merged.insert(
                    "pure_proxy_reserves".into(),
                    Value::Object(serde_json::Map::new()),
                );
            }
        }
        if let Some(Value::Array(items)) = part.get("multisig_approvals") {
            approvals.extend(items.iter().cloned());
        }
    }
    if !approvals.is_empty() {
        merged.insert("multisig_approvals".into(), Value::Array(approvals));
    }
    merged
}

#[cfg(test)]
mod merge_tests {
    use super::merge_approval_metadata;
    use serde_json::{Map, Value, json};

    fn map(value: Value) -> Map<String, Value> {
        value.as_object().cloned().unwrap_or_default()
    }

    #[test]
    fn deferring_approvals_drop_inner_obligations_and_keep_their_own() {
        let inner = map(json!({
            "pure_proxy_reserves": {"a": 5},
            "multisig_approvals": [{"payer": "inner"}],
            "proxy_for": "x",
        }));
        let outer = map(json!({
            "multisig_defers_inner": true,
            "multisig_approvals": [{"payer": "outer"}],
        }));
        let merged = merge_approval_metadata(&[&inner, &outer]);
        assert_eq!(merged["pure_proxy_reserves"], json!({}));
        assert_eq!(merged["multisig_approvals"], json!([{"payer": "outer"}]));
        assert_eq!(merged["proxy_for"], "x");
        let executing = map(json!({
            "multisig_defers_inner": false,
            "multisig_approvals": [{"payer": "outer"}],
        }));
        let merged = merge_approval_metadata(&[&inner, &executing]);
        assert_eq!(merged["pure_proxy_reserves"], json!({"a": 5}));
        assert_eq!(
            merged["multisig_approvals"],
            json!([{"payer": "inner"}, {"payer": "outer"}])
        );
        assert!(merge_approval_metadata(&[]).is_empty());
    }
}
