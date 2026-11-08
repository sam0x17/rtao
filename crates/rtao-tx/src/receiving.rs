// Derived from RaoFoundation/subtensor sdk/python/bittensor/receiving.py @ b6a81d144078eac18bd25c3af7cb31579bb670f3 (Apache-2.0); modified by rtao.
//! Receiving addresses on write paths that cannot carry a registration guard.
//!
//! A `tao_` receiving address names a hashed or post-quantum account and
//! carries the public descriptor its owner registers. A write that pays such an
//! account without registering it in the same atomic batch must not resolve
//! the address to a bare, unprotected account: it requires the descriptor's
//! registration in **finalized** state. A finalized registration is permanent
//! (it cannot be removed or have its descriptor changed), so a later reorg or a
//! disabled setup switch cannot make the resolved account lose it.

use crate::IntentError;
use rtao_client::{BlockRef, Client, ClientError, ViewExt};
use rtao_primitives::{
    AccountId,
    receiving::{Recipient, descriptor_bytes, descriptor_storage, parse_recipient},
};
use rtao_runtime::storage::hashed_accounts as st;

/// The descriptor `HashedAccounts` holds for a receiving address's account in
/// finalized state, or `None` when the account is not registered there.
///
/// A plain SS58 recipient has no descriptor and returns `None` without a
/// chain read. The registry is the descriptor's own (`Accounts` for version 1,
/// `MlDsaAccounts` for version 2), as Python's `descriptor_storage`.
pub async fn finalized_descriptor(
    client: &Client,
    recipient: &Recipient,
) -> Result<Option<Vec<u8>>, IntentError> {
    let Some(descriptor) = recipient.descriptor.as_deref() else {
        return Ok(None);
    };
    let storage = descriptor_storage(descriptor)?;
    let finalized = client.at(BlockRef::Finalized).await?;
    let record = if storage == "MlDsaAccounts" {
        finalized
            .storage(&st::ML_DSA_ACCOUNTS, (recipient.account,))
            .await?
    } else {
        finalized
            .storage(&st::ACCOUNTS, (recipient.account,))
            .await?
    };
    record
        .map(|record| {
            let record = record.to_json();
            let stored = record.get("descriptor").ok_or_else(|| {
                ClientError::Protocol("HashedAccounts record has no descriptor".into())
            })?;
            Ok::<_, IntentError>(descriptor_bytes(stored)?)
        })
        .transpose()
}

/// Python's `account_for_registered_write`: resolve a recipient for a write
/// that cannot carry a registration guard.
///
/// SS58 resolves to itself. A receiving address resolves to its account only
/// when its descriptor's registration is finalized and equals the address's
/// descriptor; an unregistered receiving address is refused, never resolved to
/// an unprotected bare account.
#[doc(alias = "bittensor.receiving.account_for_registered_write")]
pub async fn account_for_registered_write(
    client: &Client,
    address: &str,
) -> Result<AccountId, IntentError> {
    let recipient = parse_recipient(address)?;
    if recipient.descriptor.is_none() {
        return Ok(recipient.account);
    }
    match finalized_descriptor(client, &recipient).await? {
        None => Err(IntentError::validation(
            "hashed destination registration is not finalized; register the recipient and wait for finalization before using this command",
        )),
        Some(stored) if Some(stored.as_slice()) != recipient.descriptor.as_deref() => Err(
            IntentError::validation("registered hashed descriptor does not match the destination"),
        ),
        Some(_) => Ok(recipient.account),
    }
}
