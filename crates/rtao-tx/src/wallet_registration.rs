// Derived from RaoFoundation/subtensor sdk/python/bittensor/wallet_registration.py @ b6a81d144078eac18bd25c3af7cb31579bb670f3 (Apache-2.0); modified by rtao.
//! Explicit, encrypted registration of a wallet's rotating keys (D-151).
//!
//! A hashed (rotating) wallet key must be registered before it can sign:
//! the registration publishes the key's public commitments under its account.
//! A funded payer registers the wallet's missing keys through a required MEV
//! Shield submission, after approving the aggregate permanent reserve; each
//! registration is verified in finalized state. Standard post-quantum and
//! classical keys need no registration and no chain request.

use std::{fmt, sync::Arc};

use futures::future::BoxFuture;
use rtao_client::{BlockRef, ProbeSigner, ViewExt};
use rtao_primitives::{
    AccountId, AuthorityType, Balance, H256, Tao, TaoBalance,
    receiving::{descriptor_account, descriptor_storage},
};
use rtao_runtime::{
    CallBytes,
    calls::{hashed_accounts as calls, utility},
    constants::hashed_accounts as consts,
};
use rtao_signer::{DynKeySource, KeyView, SignerRole};

use crate::{ExecuteOptions, Executor, ExtrinsicResult, IntentError, hashed};

/// Python `WalletRegistrationQuote`: the missing registrations, their total
/// permanent reserve and the estimated fees.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WalletRegistrationQuote {
    /// The account that pays the reserve and fees.
    pub payer_account: AccountId,
    /// The accounts to register.
    pub accounts: Vec<AccountId>,
    /// The wallet roles among them.
    pub roles: Vec<(SignerRole, AccountId)>,
    /// The total permanent reserve.
    pub deposit: Balance<Tao>,
    /// The estimated inner and Shield carrier fees.
    pub fee: Balance<Tao>,
}

/// Python `WalletRegistrationResult`: the finalized identities, the last
/// receipt and the quoted new reserve.
#[derive(Clone, Debug)]
pub struct WalletRegistrationResult {
    /// The wallet roles that need registration, and their accounts.
    pub roles: Vec<(SignerRole, AccountId)>,
    /// Their accounts.
    pub accounts: Vec<AccountId>,
    /// The finalized block they were verified at, if any needed it.
    pub finalized_block_hash: Option<H256>,
    /// The last registration receipt, if one was submitted.
    pub result: Option<ExtrinsicResult>,
    /// The new permanent reserve.
    pub deposit: Balance<Tao>,
}

/// Python `WalletRegistrationError`: registration did not complete; the
/// wallet's keys are preserved, with any receipt and quote.
#[derive(Clone, Debug)]
pub struct WalletRegistrationError {
    /// What failed.
    pub message: String,
    /// The receipt of a submitted registration, if any.
    pub result: Option<Box<ExtrinsicResult>>,
    /// The approved quote, if one was made.
    pub quote: Option<WalletRegistrationQuote>,
}

impl fmt::Display for WalletRegistrationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Wallet keys are preserved; registration did not complete: {}",
            self.message
        )
    }
}

impl std::error::Error for WalletRegistrationError {}

/// A consent callback for the aggregate quote; it must return `true`.
pub type WalletRegistrationCallback =
    Arc<dyn Fn(WalletRegistrationQuote) -> BoxFuture<'static, bool> + Send + Sync>;

/// Options for [`Executor::register_wallet`].
#[derive(Clone)]
pub struct WalletRegistrationOptions {
    /// The roles to register (default both).
    pub roles: Vec<SignerRole>,
    /// Authorize up to this aggregate permanent reserve.
    pub max_registration_deposit: Option<Balance<Tao>>,
    /// Approve the complete quote; with an allowance, both must approve.
    pub on_registration_sponsorship: Option<WalletRegistrationCallback>,
}

impl Default for WalletRegistrationOptions {
    fn default() -> Self {
        Self {
            roles: vec![SignerRole::Coldkey, SignerRole::Hotkey],
            max_registration_deposit: None,
            on_registration_sponsorship: None,
        }
    }
}

impl fmt::Debug for WalletRegistrationOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WalletRegistrationOptions")
            .field("roles", &self.roles)
            .field("max_registration_deposit", &self.max_registration_deposit)
            .field(
                "on_registration_sponsorship",
                &self.on_registration_sponsorship.as_ref().map(|_| ".."),
            )
            .finish()
    }
}

/// Wallet roles and their accounts.
pub type RegistrationRoles = Vec<(SignerRole, AccountId)>;
/// Accounts and the descriptors they register.
pub type RegistrationDescriptors = Vec<(AccountId, Vec<u8>)>;

fn role_account(keys: &dyn KeyView, role: SignerRole) -> Result<AccountId, IntentError> {
    Ok(match role {
        SignerRole::Coldkey => keys.coldkey_address()?,
        SignerRole::Hotkey => keys.hotkey_address()?,
    })
}

/// Python `registration_targets`: the wallet roles whose keys need
/// registration, from public keys only, each descriptor checked against its
/// account before any chain request.
#[doc(alias = "bittensor.wallet_registration.registration_targets")]
pub fn registration_targets(
    wallet: &dyn KeyView,
    roles: &[SignerRole],
) -> Result<(RegistrationRoles, RegistrationDescriptors), IntentError> {
    let mut selected = Vec::new();
    let mut descriptors: Vec<(AccountId, Vec<u8>)> = Vec::new();
    let mut seen = Vec::new();
    for role in roles {
        if seen.contains(role) {
            continue;
        }
        seen.push(*role);
        let Some(descriptor) = wallet.authority_descriptor(*role) else {
            continue;
        };
        if !AuthorityType::from_descriptor(&descriptor)
            .is_some_and(AuthorityType::requires_registration)
        {
            continue;
        }
        let account = role_account(wallet, *role)?;
        if descriptor_account(&descriptor)? != account {
            return Err(IntentError::validation(format!(
                "{} descriptor does not match its account",
                role.as_str()
            )));
        }
        match descriptors
            .iter()
            .find(|(existing, _)| *existing == account)
        {
            Some((_, existing)) if *existing != descriptor => {
                return Err(IntentError::validation(
                    "wallet roles have conflicting account descriptors",
                ));
            }
            Some(_) => {}
            None => descriptors.push((account, descriptor)),
        }
        selected.push((*role, account));
    }
    Ok((selected, descriptors))
}

/// Python `registration_deposit_for_types`: the reserve new keys of these
/// schemes will need, quoted before they are generated; schemes needing no
/// registration need no chain request.
#[doc(alias = "bittensor.wallet_registration.registration_deposit_for_types")]
pub async fn registration_deposit_for(
    executor: &Executor,
    kinds: &[AuthorityType],
) -> Result<Balance<Tao>, IntentError> {
    let kinds: Vec<_> = kinds
        .iter()
        .copied()
        .filter(|kind| kind.requires_registration())
        .collect();
    if kinds.is_empty() {
        return Ok(Balance::from_rao(0));
    }
    let view = executor.client().at(BlockRef::Best).await?;
    let runtime = view.block().runtime.clone();
    let mut total = 0u64;
    for kind in kinds {
        let legacy = if kind.is_post_quantum() {
            &consts::ML_DSA_REGISTRATION_DEPOSIT
        } else {
            &consts::REGISTRATION_DEPOSIT
        };
        let quote = hashed::quote_registration_deposit(&view, &runtime, legacy).await?;
        total = total
            .checked_add(quote.amount_rao)
            .ok_or(rtao_primitives::UnitError::Overflow)?;
    }
    Ok(Balance::from_rao(total))
}

fn failed(
    message: impl fmt::Display,
    result: Option<ExtrinsicResult>,
    quote: Option<WalletRegistrationQuote>,
) -> WalletRegistrationError {
    WalletRegistrationError {
        message: message.to_string(),
        result: result.map(Box::new),
        quote,
    }
}

impl Executor {
    /// Python `Client.register_wallet`: register the wallet's rotating keys
    /// with a funded payer through a required MEV Shield submission.
    ///
    /// An allowance authorizes the aggregate permanent reserve; a callback
    /// sees the complete quote; with both, both must approve, and with
    /// neither nothing is submitted. Every registration is verified in
    /// finalized state; classical registrations go one per transaction, each
    /// verified before the next. Keys already registered are skipped, so a
    /// retry after an error completes the rest. No public-route fallback.
    #[doc(alias = "bittensor.wallet_registration.register_wallet")]
    pub async fn register_wallet(
        &self,
        wallet: &dyn KeyView,
        payer: &dyn DynKeySource,
        opts: &WalletRegistrationOptions,
    ) -> Result<WalletRegistrationResult, Box<WalletRegistrationError>> {
        let mut result = None;
        let mut quote = None;
        match self
            .register_wallet_inner(wallet, payer, opts, &mut result, &mut quote)
            .await
        {
            Ok(done) => Ok(done),
            Err(error) => Err(Box::new(failed(error, result, quote))),
        }
    }

    async fn register_wallet_inner(
        &self,
        wallet: &dyn KeyView,
        payer: &dyn DynKeySource,
        opts: &WalletRegistrationOptions,
        result: &mut Option<ExtrinsicResult>,
        quote_out: &mut Option<WalletRegistrationQuote>,
    ) -> Result<WalletRegistrationResult, crate::TxError> {
        let (selected, descriptors) = registration_targets(wallet, &opts.roles)?;
        if descriptors.is_empty() {
            return Ok(WalletRegistrationResult {
                roles: Vec::new(),
                accounts: Vec::new(),
                finalized_block_hash: None,
                result: None,
                deposit: Balance::from_rao(0),
            });
        }
        let client = self.client();
        let finalized = client.at(BlockRef::Finalized).await?;
        let mut missing = Vec::new();
        for (account, descriptor) in &descriptors {
            if !registered(&finalized, *account, descriptor).await? {
                missing.push((*account, descriptor.clone()));
            }
        }
        let accounts: Vec<AccountId> = descriptors.iter().map(|(account, _)| *account).collect();
        if missing.is_empty() {
            return Ok(WalletRegistrationResult {
                roles: selected,
                accounts,
                finalized_block_hash: Some(finalized.block().hash),
                result: None,
                deposit: Balance::from_rao(0),
            });
        }
        let payer_account = payer
            .coldkey_address()
            .or_else(|error| payer.bare_signer_account().ok_or(error))
            .map_err(IntentError::from)?;
        if missing.iter().any(|(account, _)| *account == payer_account) {
            return Err(IntentError::validation(
                "an unregistered wallet cannot pay for its own registration",
            )
            .into());
        }
        let payer_descriptor = payer.authority_descriptor(SignerRole::Coldkey);
        if let Some(descriptor) = payer_descriptor.as_deref()
            && AuthorityType::from_descriptor(descriptor)
                .is_some_and(AuthorityType::requires_registration)
            && !registered(&finalized, payer_account, descriptor).await?
        {
            return Err(IntentError::validation(
                "registration payer must already be registered in finalized state",
            )
            .into());
        }
        let head = client.at(BlockRef::Best).await?;
        let runtime = head.block().runtime.clone();
        for (_, descriptor) in &missing {
            if hashed::descriptor_registration_enabled(&head, &runtime, descriptor).await?
                != Some(true)
            {
                return Err(IntentError::validation(
                    "hashed wallet registration is not enabled for this scheme",
                )
                .into());
            }
        }
        if runtime.storage_entry("MevShield", "NextKey").is_none()
            || head
                .storage(&rtao_runtime::storage::mev_shield::NEXT_KEY, ())
                .await?
                .is_none_or(|key| key.is_empty())
        {
            return Err(IntentError::validation(
                "MEV Shield is unavailable; wallet registration was not submitted",
            )
            .into());
        }
        // The classical runtime hook accepts one registration, optionally
        // guarded, but not another registration as its batch follow-up.
        let groups: Vec<RegistrationDescriptors> = if missing
            .iter()
            .any(|(_, descriptor)| descriptor_storage(descriptor).ok() == Some("Accounts"))
        {
            missing.iter().map(|entry| vec![entry.clone()]).collect()
        } else {
            vec![missing.clone()]
        };
        let probe_kind = payer_descriptor.as_deref().and_then(|descriptor| {
            AuthorityType::from_descriptor(descriptor).map(|kind| (kind, descriptor))
        });
        let probe = match probe_kind {
            Some((kind, descriptor)) => ProbeSigner::Authority { kind, descriptor },
            None => ProbeSigner::Classical(rtao_primitives::CryptoType::Sr25519),
        };
        let period = rtao_catalog::py::MEV_SHIELD_ERA_PERIOD;
        let mut total = 0u64;
        let mut fee = 0u64;
        let mut transactions: Vec<(CallBytes, RegistrationDescriptors)> = Vec::new();
        for group in groups {
            let mut registrations = Vec::new();
            let mut ceiling: Option<u64> = None;
            for (_, descriptor) in &group {
                let legacy = if descriptor[0] == 2 {
                    &consts::ML_DSA_REGISTRATION_DEPOSIT
                } else {
                    &consts::REGISTRATION_DEPOSIT
                };
                let quote = hashed::quote_registration_deposit(&head, &runtime, legacy).await?;
                total = total
                    .checked_add(quote.amount_rao)
                    .ok_or(rtao_primitives::UnitError::Overflow)
                    .map_err(IntentError::from)?;
                if quote.configurable {
                    ceiling = Some(ceiling.map_or(quote.amount_rao, |c| c.min(quote.amount_rao)));
                }
                registrations.push(
                    calls::register(hashed::descriptor_call_value(descriptor)?)
                        .compose(&runtime)
                        .map_err(IntentError::from)?,
                );
            }
            if let Some(ceiling) = ceiling {
                // Each transaction binds the approved per-account reserve,
                // even if sudo raises it between sequential registrations.
                registrations.insert(
                    0,
                    calls::check_registration_deposit(TaoBalance(ceiling))
                        .compose(&runtime)
                        .map_err(IntentError::from)?,
                );
            }
            let composed = if registrations.len() == 1 {
                registrations.remove(0)
            } else {
                utility::batch_all(registrations)
                    .compose(&runtime)
                    .map_err(IntentError::from)?
            };
            fee = fee
                .saturating_add(
                    client
                        .estimate_weight_as(composed.clone(), payer_account, probe)
                        .await?
                        .partial_fee,
                )
                .saturating_add(
                    client
                        .estimate_shielded_carrier_fee_as(
                            payer_account,
                            probe,
                            composed.clone(),
                            period,
                        )
                        .await?
                        .rao(),
                );
            transactions.push((composed, group));
        }
        let quote = WalletRegistrationQuote {
            payer_account,
            accounts: missing.iter().map(|(account, _)| *account).collect(),
            roles: selected
                .iter()
                .filter(|(_, account)| missing.iter().any(|(missing, _)| missing == account))
                .copied()
                .collect(),
            deposit: Balance::from_rao(total),
            fee: Balance::from_rao(fee),
        };
        *quote_out = Some(quote.clone());
        if let Some(maximum) = opts.max_registration_deposit
            && total > maximum.rao()
        {
            return Err(IntentError::validation(
                "wallet registration reserve exceeds the approved maximum",
            )
            .into());
        }
        if opts.max_registration_deposit.is_none() && opts.on_registration_sponsorship.is_none() {
            return Err(IntentError::validation(
                "wallet registration requires max_registration_deposit or on_registration_sponsorship approval",
            )
            .into());
        }
        let account = head
            .storage(&rtao_runtime::storage::system::ACCOUNT, (payer_account,))
            .await?
            .to_json();
        let data = account.get("data").cloned().unwrap_or_default();
        let field = |name: &str| {
            data.get(name)
                .and_then(serde_json::Value::as_u64)
                .unwrap_or_default()
        };
        let minimum = head
            .constant(&rtao_runtime::constants::balances::EXISTENTIAL_DEPOSIT)
            .await?
            .0;
        let required = total
            .saturating_add(fee)
            .saturating_add(field("frozen").max(minimum));
        if field("free") < required {
            return Err(IntentError::validation(
                "registration payer has insufficient spendable balance for the permanent reserve, inner transaction fee and MEV Shield carrier fee",
            )
            .into());
        }
        if let Some(callback) = &opts.on_registration_sponsorship
            && !callback(quote.clone()).await
        {
            return Err(
                IntentError::validation("wallet registration sponsorship was declined").into(),
            );
        }
        let mut last_finalized = finalized.block().hash;
        for (composed, group) in transactions {
            *result = None;
            let receipt = self
                .submit_call_dyn(
                    composed,
                    payer,
                    SignerRole::Coldkey,
                    &ExecuteOptions {
                        shielded: true,
                        wait_for_inclusion: true,
                        wait_for_finalization: true,
                        ..Default::default()
                    },
                )
                .await?;
            let success = receipt.success;
            let message = receipt.message.clone();
            *result = Some(receipt);
            if !success {
                return Err(IntentError::validation(message).into());
            }
            let finalized = client.at(BlockRef::Finalized).await?;
            last_finalized = finalized.block().hash;
            for (account, descriptor) in &group {
                if !registered(&finalized, *account, descriptor).await? {
                    return Err(IntentError::validation(
                        "wallet registration is not present in finalized state",
                    )
                    .into());
                }
            }
        }
        let finalized = client.at(BlockRef::Finalized).await?;
        for (account, descriptor) in &descriptors {
            if !registered(&finalized, *account, descriptor).await? {
                return Err(IntentError::validation(
                    "wallet registration is not present in finalized state",
                )
                .into());
            }
        }
        let _ = last_finalized;
        Ok(WalletRegistrationResult {
            roles: selected,
            accounts,
            finalized_block_hash: Some(finalized.block().hash),
            result: result.clone(),
            deposit: quote.deposit,
        })
    }
}

async fn registered(
    view: &rtao_client::Snapshot,
    account: AccountId,
    descriptor: &[u8],
) -> Result<bool, IntentError> {
    match hashed::registered_record(view, account, descriptor).await? {
        None => Ok(false),
        Some(stored) if stored != descriptor => Err(IntentError::validation(
            "registered wallet descriptor does not match the local key",
        )),
        Some(_) => Ok(true),
    }
}
