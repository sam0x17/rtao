// Derived from RaoFoundation/subtensor sdk/python/bittensor/hashed.py @ b6a81d144078eac18bd25c3af7cb31579bb670f3 (Apache-2.0); modified by rtao.
//! Protected (post-quantum) account records and the shared registration reserve.
//!
//! These are the reads Python's `hashed` module makes before composing a call
//! that establishes a protected account, such as a pure proxy spawned by a
//! quantum-resistant payer. They query live metadata and pinned state on every
//! call: sudo can change the reserve without changing the runtime.

use rtao_client::{View, ViewExt};
use rtao_primitives::{AccountId, TaoBalance};
use rtao_runtime::{
    ConstantDescriptor, RuntimeHandle, constants::hashed_accounts as consts,
    storage::hashed_accounts as st,
};
use rtao_tx::IntentError;

/// Which permanent registry protects an account.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProtectedKind {
    /// A version-1 hashed account (`HashedAccounts.Accounts`).
    V1,
    /// A direct ML-DSA account (`HashedAccounts.MlDsaAccounts`).
    MlDsa,
    /// A derived authority policy (`HashedAccounts.DerivedAuthorities`).
    Derived,
}

fn has_constant(runtime: &RuntimeHandle, pallet: &str, name: &str) -> bool {
    runtime.core().constant(pallet, name).is_some()
}

/// Resolve direct and derived protection without querying absent post-quantum metadata.
#[doc(alias = "bittensor.hashed.protected_account_record")]
pub(crate) async fn protected_account_kind<V: View + ?Sized>(
    view: &V,
    runtime: &RuntimeHandle,
    account: AccountId,
) -> Result<Option<ProtectedKind>, IntentError> {
    if runtime
        .storage_entry("HashedAccounts", "Accounts")
        .is_none()
    {
        // Runtimes before the hashed-account pallet protect nothing.
        return Ok(None);
    }
    if view.storage(&st::ACCOUNTS, (account,)).await?.is_some() {
        return Ok(Some(ProtectedKind::V1));
    }
    if has_constant(runtime, "HashedAccounts", "MlDsaRegistrationDeposit") {
        if view
            .storage(&st::ML_DSA_ACCOUNTS, (account,))
            .await?
            .is_some()
        {
            return Ok(Some(ProtectedKind::MlDsa));
        }
        if view
            .storage(&st::DERIVED_AUTHORITIES, (account,))
            .await?
            .is_some()
        {
            return Ok(Some(ProtectedKind::Derived));
        }
    }
    Ok(None)
}

/// Read the permanent direct or derived quantum-resistant authority floor.
#[doc(alias = "bittensor.hashed.is_pq_required")]
pub(crate) async fn is_pq_required<V: View + ?Sized>(
    view: &V,
    runtime: &RuntimeHandle,
    account: AccountId,
) -> Result<bool, IntentError> {
    if !has_constant(runtime, "HashedAccounts", "MlDsaRegistrationDeposit") {
        return Ok(false);
    }
    Ok(matches!(
        protected_account_kind(view, runtime, account).await?,
        Some(ProtectedKind::MlDsa | ProtectedKind::Derived)
    ))
}

/// The live shared registration reserve and whether a call may bound it.
#[doc(alias = "bittensor.hashed.HashedRegistrationDepositQuote")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DepositQuote {
    /// Reserve in rao, possibly zero.
    pub amount_rao: u64,
    /// The runtime stores a sudo-configurable reserve (and accepts its guard).
    pub configurable: bool,
}

/// Quote the live shared reserve at the pinned state, including zero.
///
/// Only runtimes whose metadata lacks the storage entry use the older
/// per-kind constant; a failed read never turns into an approved cost.
#[doc(alias = "bittensor.hashed.quote_hashed_registration_deposit")]
pub(crate) async fn quote_registration_deposit<V: View + ?Sized>(
    view: &V,
    runtime: &RuntimeHandle,
    legacy: &ConstantDescriptor<TaoBalance>,
) -> Result<DepositQuote, IntentError> {
    let (amount, configurable) = if runtime
        .storage_entry("HashedAccounts", "HashedAccountRegistrationDeposit")
        .is_some()
    {
        (
            view.storage(&st::HASHED_ACCOUNT_REGISTRATION_DEPOSIT, ())
                .await?,
            true,
        )
    } else if has_constant(runtime, legacy.surface().pallet, legacy.surface().name) {
        (view.constant(legacy).await?, false)
    } else {
        return Err(IntentError::validation(
            "could not quote hashed account registration reserve",
        ));
    };
    Ok(DepositQuote {
        amount_rao: amount.0,
        configurable,
    })
}

/// The derived-account (pure proxy) reserve constant used by older runtimes.
pub(crate) const DERIVED_REGISTRATION_DEPOSIT: &ConstantDescriptor<TaoBalance> =
    &consts::DERIVED_REGISTRATION_DEPOSIT;
