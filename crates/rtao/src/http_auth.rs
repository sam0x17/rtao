// Derived from RaoFoundation/subtensor sdk/python/bittensor/http_auth.py @ b6a81d144078eac18bd25c3af7cb31579bb670f3 (Apache-2.0); modified by rtao.
//! btauth signing for hashed hotkeys from finalized chain state (D-153).

use std::sync::Arc;

use rtao_btauth::{AuthError, Clock, HashedHttpSigner};
use rtao_client::{BlockRef, Client, ClientError, ViewExt};
use rtao_primitives::{AccountId, receiving::descriptor_bytes};
use rtao_signer::Signer;

/// A hotkey ready for btauth signing: classical, or hashed at its finalized
/// generation (Python `prepare_signer`'s result).
pub enum PreparedHttpSigner {
    /// An Ed25519 or Sr25519 signer.
    Classical(Arc<dyn Signer>),
    /// A hashed SR/ED signer at a finalized generation of at least one.
    Hashed(HashedHttpSigner),
}

impl PreparedHttpSigner {
    /// The signing account.
    pub fn account(&self) -> AccountId {
        match self {
            Self::Classical(signer) => signer.account(),
            Self::Hashed(signer) => signer.account(),
        }
    }
    /// Sign a request ([`rtao_btauth::sign`] or [`rtao_btauth::sign_hashed`]).
    pub async fn sign(
        &self,
        method: &http::Method,
        target: &str,
        body: &[u8],
        receiver: Option<&AccountId>,
        now: &dyn Clock,
    ) -> Result<http::HeaderMap, AuthError> {
        match self {
            Self::Classical(signer) => {
                rtao_btauth::sign(signer.as_ref(), method, target, body, receiver, now).await
            }
            Self::Hashed(signer) => {
                rtao_btauth::sign_hashed(signer, method, target, body, receiver, now).await
            }
        }
    }
}

/// Python `http_auth.prepare_signer`: prepare a hotkey for btauth signing.
///
/// A classical signer needs no chain access. A hashed SR/ED signer's
/// signatures reveal its original identity key, so its registration must
/// exist in finalized state with the signer's descriptor and generation zero
/// must already be retired; it then signs at that finalized generation.
/// Post-quantum HTTP signing is not supported.
#[doc(alias = "bittensor.http_auth.prepare_signer")]
pub async fn prepare_signer(
    client: &Client,
    signer: Arc<dyn Signer>,
) -> Result<PreparedHttpSigner, ClientError> {
    let refuse = |reason: &str| ClientError::SigningDisabled {
        reason: reason.to_owned(),
    };
    let Some(authority) = signer.authority() else {
        return Ok(PreparedHttpSigner::Classical(signer));
    };
    if authority.authority_type().is_post_quantum() {
        return Err(refuse("post-quantum HTTP signing is not supported"));
    }
    let account = authority.account();
    let descriptor = authority.descriptor().to_vec();
    let finalized = client.at(BlockRef::Finalized).await?;
    let record = finalized
        .storage(
            &rtao_runtime::storage::hashed_accounts::ACCOUNTS,
            (account,),
        )
        .await?
        .map(|record| record.to_json());
    let Some(record) = record.filter(|record| {
        record
            .get("descriptor")
            .and_then(|stored| descriptor_bytes(stored).ok())
            .is_some_and(|stored| stored == descriptor)
    }) else {
        return Err(refuse(
            "hashed hotkey is not registered on the selected chain",
        ));
    };
    let generation = record
        .get("generation")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| refuse("hashed hotkey is not registered on the selected chain"))?;
    if generation < 1 {
        return Err(refuse(
            "hashed HTTP signing requires generation zero to be retired by a finalized on-chain transaction first",
        ));
    }
    HashedHttpSigner::new(signer, generation)
        .map(PreparedHttpSigner::Hashed)
        .map_err(ClientError::Signing)
}
