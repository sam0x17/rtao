// Derived from RaoFoundation/subtensor sdk/python/bittensor/http_auth.py @ b6a81d144078eac18bd25c3af7cb31579bb670f3 (Apache-2.0); modified by rtao.
//! Trusted btauth sender schemes from finalized chain state (D-172).
//!
//! Python's `http_auth.verify` refuses a sender without a trusted scheme and
//! leaves the mapping to the application ("configuration or a registry, never
//! the request"). A miner's callers are metagraph hotkeys, so the registry is
//! the chain: a hotkey whose account has a finalized `HashedAccounts.Accounts`
//! record is the hashed SR/ED scheme its stored descriptor names; a hotkey
//! with none is a classical key, which rtao takes to be Sr25519 (btcli's
//! hotkey default). A classical Ed25519 sender needs a trusted override. A
//! rotating post-quantum account (`HashedAccounts.MlDsaAccounts`) and any
//! sender outside the metagraph fail closed.

use rtao_btauth::{SenderSchemes, TrustedSchemes};
use rtao_client::{BlockRef, Client, ClientError, ViewExt};
use rtao_primitives::{
    AccountId, AuthorityType,
    receiving::{descriptor_account, descriptor_bytes},
};
use rtao_runtime::storage::hashed_accounts;
use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};

/// Btcli's default hotkey scheme for an unhashed (classical) metagraph hotkey.
const CLASSICAL_DEFAULT: u8 = 1;

/// Sender schemes of metagraph hotkeys, read from finalized chain state
/// (D-172), with optional operator-trusted overrides that take precedence.
///
/// [`crate::MinerRuntime`] uses it by default and refreshes it whenever the
/// metagraph changes and every 50 finalized blocks. The scheme is never taken
/// from a request header: a request declaring another scheme is refused.
#[derive(Default)]
pub struct FinalizedRegistrySchemes {
    trusted: Option<TrustedSchemes>,
    schemes: RwLock<Arc<HashMap<AccountId, u8>>>,
}

impl std::fmt::Debug for FinalizedRegistrySchemes {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FinalizedRegistrySchemes")
            .field("trusted", &self.trusted.is_some())
            .field("senders", &self.load().len())
            .finish()
    }
}

impl FinalizedRegistrySchemes {
    /// An empty registry: every sender fails closed until [`Self::refresh`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Operator-trusted schemes (exact SS58 text) that take precedence over
    /// the chain registry, for example a classical Ed25519 validator.
    pub fn with_trusted(mut self, trusted: TrustedSchemes) -> Self {
        self.trusted = Some(trusted);
        self
    }

    fn load(&self) -> Arc<HashMap<AccountId, u8>> {
        self.schemes
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// The scheme recorded for `account` by the last refresh.
    pub fn recorded(&self, account: &AccountId) -> Option<u8> {
        self.load().get(account).copied()
    }

    /// Replace the mapping with the schemes of `hotkeys` at the finalized
    /// block. On a read error the previous mapping is kept.
    pub async fn refresh(&self, client: &Client, hotkeys: &[AccountId]) -> Result<(), ClientError> {
        let finalized = client.at(BlockRef::Finalized).await?;
        let classical = finalized
            .fetch(
                hotkeys
                    .iter()
                    .map(|hotkey| hashed_accounts::ACCOUNTS.at((*hotkey,)))
                    .collect::<Vec<_>>(),
            )
            .await?;
        let post_quantum = finalized
            .fetch(
                hotkeys
                    .iter()
                    .map(|hotkey| hashed_accounts::ML_DSA_ACCOUNTS.at((*hotkey,)))
                    .collect::<Vec<_>>(),
            )
            .await?;
        let mut schemes = HashMap::with_capacity(hotkeys.len());
        for ((hotkey, hashed), pq) in hotkeys.iter().zip(classical).zip(post_quantum) {
            if pq.is_some() {
                // Post-quantum HTTP authentication is not supported (D-174).
                continue;
            }
            let Some(record) = hashed else {
                schemes.insert(*hotkey, CLASSICAL_DEFAULT);
                continue;
            };
            let descriptor = record
                .to_json()
                .get("descriptor")
                .and_then(|stored| descriptor_bytes(stored).ok());
            // The record must name a hashed SR/ED descriptor whose account is
            // this hotkey; anything else fails closed.
            let scheme = descriptor.and_then(|descriptor| {
                let account = descriptor_account(&descriptor).ok()?;
                let scheme = AuthorityType::from_descriptor(&descriptor)?;
                (account == *hotkey && !scheme.is_post_quantum()).then_some(scheme.code())
            });
            if let Some(scheme) = scheme {
                schemes.insert(*hotkey, scheme);
            }
        }
        *self
            .schemes
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Arc::new(schemes);
        Ok(())
    }
}

impl SenderSchemes for FinalizedRegistrySchemes {
    fn scheme(&self, sender: &str, declared: u8) -> Option<u8> {
        if let Some(trusted) = &self.trusted
            && let Some(scheme) = trusted.scheme(sender, declared)
        {
            return Some(scheme);
        }
        let account: AccountId = sender.parse().ok()?;
        self.recorded(&account)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]
    use super::*;
    use rtao_client::{ClientConfig, Network};
    use rtao_core::keys::{CRYPTO_HASHED, CRYPTO_HASHED_ED25519, CRYPTO_MLDSA65, Keypair};
    use rtao_primitives::receiving::descriptor_value;
    use rtao_testkit::{FakeChain, FakeState, rpc::RpcClient};
    use serde_json::json;

    fn descriptor(seed: u8, crypto: u8) -> (AccountId, Vec<u8>) {
        let key = Keypair::from_seed(&[seed; 32], crypto).expect("keypair");
        let descriptor = key.authorization_descriptor().expect("descriptor");
        (
            descriptor_account(&descriptor).expect("account"),
            descriptor,
        )
    }

    #[tokio::test]
    async fn schemes_come_from_finalized_records_and_overrides() -> anyhow::Result<()> {
        let classical = AccountId([1; 32]);
        let ed = AccountId([2; 32]);
        let (hashed_sr, sr_descriptor) = descriptor(3, CRYPTO_HASHED);
        let (hashed_ed, ed_descriptor) = descriptor(4, CRYPTO_HASHED_ED25519);
        let (pq, pq_descriptor) = descriptor(5, CRYPTO_MLDSA65);
        let (mismatched, _) = descriptor(6, CRYPTO_HASHED);
        let outsider = AccountId([9; 32]);
        let mut state = FakeState::try_default()?;
        for (account, descriptor, storage) in [
            (hashed_sr, &sr_descriptor, "Accounts"),
            (hashed_ed, &ed_descriptor, "Accounts"),
            (pq, &pq_descriptor, "MlDsaAccounts"),
            // A record whose descriptor names another account.
            (mismatched, &sr_descriptor, "Accounts"),
        ] {
            state.seed_python(
                "HashedAccounts",
                storage,
                &[json!(account.to_string())],
                json!({
                    "descriptor": descriptor_value(descriptor)?,
                    "generation": 1,
                    "commitment": format!(
                        "0x{}",
                        hex::encode(vec![7u8; if storage == "Accounts" { 32 } else { 64 }])
                    ),
                }),
            )?;
        }
        let chain = FakeChain::from_state(state);
        let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
        config.track_heads = false;
        config.runtime_cache_dir = None;
        config.config_store = None;
        let client = Client::from_rpc(RpcClient::new(chain), config).await?;
        let registry = FinalizedRegistrySchemes::new()
            .with_trusted(TrustedSchemes::new([(ed.to_string(), 0)])?);
        // Empty until refreshed: fail closed.
        assert_eq!(registry.scheme(&classical.to_string(), 1), None);
        registry
            .refresh(
                &client,
                &[classical, ed, hashed_sr, hashed_ed, pq, mismatched],
            )
            .await?;
        assert_eq!(registry.scheme(&classical.to_string(), 1), Some(1));
        // The declared scheme never changes the trusted one.
        assert_eq!(registry.scheme(&classical.to_string(), 0), Some(1));
        assert_eq!(registry.scheme(&ed.to_string(), 0), Some(0));
        assert_eq!(registry.scheme(&hashed_sr.to_string(), 1), Some(4));
        assert_eq!(registry.scheme(&hashed_ed.to_string(), 1), Some(6));
        assert_eq!(registry.scheme(&pq.to_string(), 7), None);
        assert_eq!(registry.scheme(&mismatched.to_string(), 4), None);
        assert_eq!(registry.scheme(&outsider.to_string(), 1), None);
        assert_eq!(registry.scheme("not an address", 1), None);
        client.close().await;
        Ok(())
    }
}
