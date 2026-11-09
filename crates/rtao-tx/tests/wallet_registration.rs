// Derived from RaoFoundation/subtensor sdk/python/bittensor/wallet_registration.py @ b6a81d144078eac18bd25c3af7cb31579bb670f3 (Apache-2.0); modified by rtao.
//! Wallet registration refuses before anything is signed or submitted
//! unless the quote is complete, approved and funded (D-151).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use rtao_client::{Client, ClientConfig, Network};
use rtao_core::keys::{CRYPTO_HASHED, CRYPTO_MLDSA65_FIXED, Keypair};
use rtao_primitives::{AccountId, Balance, CryptoType, receiving::descriptor_value};
use rtao_signer::{
    AuthoritySigner, BoxFuture, DynKeySource, KeyView, KeypairAuthoritySigner, KeypairSigner,
    Signer, SignerError, SignerRole,
};
use rtao_testkit::{FakeChain, FakeState, rpc::RpcClient};
use rtao_tx::{
    Executor,
    wallet_registration::{WalletRegistrationOptions, registration_targets},
};
use serde_json::json;

const TAO: u64 = 1_000_000_000;
const DEPOSIT: u64 = 3 * TAO;

/// A wallet whose coldkey is a hashed key (public only) and hotkey classical.
struct HashedWallet {
    cold: KeypairAuthoritySigner,
    hot: AccountId,
}
impl KeyView for HashedWallet {
    fn hotkey_address(&self) -> Result<AccountId, SignerError> {
        Ok(self.hot)
    }
    fn hotkey_public_key(&self) -> Result<[u8; 32], SignerError> {
        Ok(self.hot.0)
    }
    fn coldkey_address(&self) -> Result<AccountId, SignerError> {
        Ok(AuthoritySigner::account(&self.cold))
    }
    fn authority_descriptor(&self, role: SignerRole) -> Option<Vec<u8>> {
        (role == SignerRole::Coldkey).then(|| self.cold.descriptor().to_vec())
    }
}

struct Payer {
    signer: KeypairSigner,
    unlocks: AtomicUsize,
}
impl KeyView for Payer {
    fn hotkey_address(&self) -> Result<AccountId, SignerError> {
        Err(SignerError::KeyUnavailable(SignerRole::Hotkey))
    }
    fn hotkey_public_key(&self) -> Result<[u8; 32], SignerError> {
        Err(SignerError::KeyUnavailable(SignerRole::Hotkey))
    }
    fn coldkey_address(&self) -> Result<AccountId, SignerError> {
        Ok(self.signer.account())
    }
}
impl DynKeySource for Payer {
    fn signer_for<'a>(
        &'a self,
        _role: SignerRole,
    ) -> BoxFuture<'a, Result<Arc<dyn Signer>, SignerError>> {
        Box::pin(async move {
            self.unlocks.fetch_add(1, Ordering::SeqCst);
            Ok(Arc::new(self.signer.clone()) as Arc<dyn Signer>)
        })
    }
}

fn wallet() -> HashedWallet {
    HashedWallet {
        cold: KeypairAuthoritySigner::new(Keypair::from_seed(&[4; 32], CRYPTO_HASHED).unwrap())
            .unwrap(),
        hot: AccountId([9; 32]),
    }
}

fn payer() -> Payer {
    Payer {
        signer: KeypairSigner::from_seed(&[5; 32], CryptoType::Sr25519).unwrap(),
        unlocks: AtomicUsize::new(0),
    }
}

async fn fixture(
    wallet: &HashedWallet,
    payer: &Payer,
    payer_free: u64,
    registered: bool,
) -> (Executor, FakeChain) {
    let mut state = FakeState::try_default().unwrap();
    state.fee = 50;
    state
        .seed_python("AdminUtils", "HashedAccountsEnabled", &[], json!(true))
        .unwrap();
    state
        .seed_python(
            "HashedAccounts",
            "HashedAccountRegistrationDeposit",
            &[],
            json!(DEPOSIT),
        )
        .unwrap();
    state
        .seed_typed("MevShield", "NextKey", (), vec![0u8; 1184])
        .unwrap();
    state
        .seed_python(
            "System",
            "Account",
            &[json!(payer.signer.account().to_string())],
            json!({"data": {"free": payer_free}}),
        )
        .unwrap();
    if registered {
        let account = AuthoritySigner::account(&wallet.cold);
        state
            .seed_python(
                "HashedAccounts",
                "Accounts",
                &[json!(account.to_string())],
                json!({
                    "descriptor": descriptor_value(wallet.cold.descriptor()).unwrap(),
                    "generation": 0,
                    "commitment": format!("0x{}", hex::encode(wallet.cold.commitment(0).unwrap())),
                }),
            )
            .unwrap();
    }
    let chain = FakeChain::from_state(state);
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None).unwrap()).unwrap();
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    let client = Client::from_rpc(RpcClient::new(chain.clone()), config)
        .await
        .unwrap();
    (Executor::new(client), chain)
}

#[test]
fn only_rotating_keys_are_registration_targets() {
    let wallet = wallet();
    let (roles, descriptors) =
        registration_targets(&wallet, &[SignerRole::Coldkey, SignerRole::Hotkey]).unwrap();
    assert_eq!(
        roles,
        vec![(SignerRole::Coldkey, AuthoritySigner::account(&wallet.cold))]
    );
    assert_eq!(descriptors.len(), 1);
    // A standard post-quantum key needs no registration.
    let standard = HashedWallet {
        cold: KeypairAuthoritySigner::new(
            Keypair::from_seed(&[4; 32], CRYPTO_MLDSA65_FIXED).unwrap(),
        )
        .unwrap(),
        hot: AccountId([9; 32]),
    };
    let (roles, _) = registration_targets(&standard, &[SignerRole::Coldkey]).unwrap();
    assert!(roles.is_empty());
}

#[tokio::test]
async fn registration_refuses_without_an_approved_and_funded_quote() {
    let wallet = wallet();
    let payer = payer();
    let (executor, chain) = fixture(&wallet, &payer, 100 * TAO, false).await;
    let error = executor
        .register_wallet(&wallet, &payer, &WalletRegistrationOptions::default())
        .await
        .unwrap_err();
    assert!(
        error.message.contains("requires max_registration_deposit"),
        "{error}"
    );
    let quote = error.quote.clone().expect("quote");
    assert_eq!(quote.deposit, Balance::from_rao(DEPOSIT));
    assert_eq!(quote.accounts, vec![AuthoritySigner::account(&wallet.cold)]);
    assert!(quote.fee.rao() > 0);
    assert!(error.to_string().starts_with("Wallet keys are preserved"));
    let error = executor
        .register_wallet(
            &wallet,
            &payer,
            &WalletRegistrationOptions {
                max_registration_deposit: Some(Balance::from_rao(DEPOSIT - 1)),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert!(
        error.message.contains("exceeds the approved maximum"),
        "{error}"
    );
    let error = executor
        .register_wallet(
            &wallet,
            &payer,
            &WalletRegistrationOptions {
                on_registration_sponsorship: Some(Arc::new(|_| Box::pin(async { false }))),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert!(error.message.contains("declined"), "{error}");
    // The wallet cannot pay for its own registration.
    let error = executor
        .register_wallet(&wallet, &wallet.cold, &WalletRegistrationOptions::default())
        .await
        .unwrap_err();
    assert!(
        error
            .message
            .contains("cannot pay for its own registration"),
        "{error}"
    );
    assert_eq!(payer.unlocks.load(Ordering::SeqCst), 0);
    assert!(chain.mutate_state(|state| state.submissions.is_empty()));
    executor.client().close().await;

    let (executor, chain) = fixture(&wallet, &payer, DEPOSIT, false).await;
    let error = executor
        .register_wallet(
            &wallet,
            &payer,
            &WalletRegistrationOptions {
                max_registration_deposit: Some(Balance::from_rao(DEPOSIT)),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert!(
        error.message.contains("insufficient spendable balance"),
        "{error}"
    );
    assert_eq!(payer.unlocks.load(Ordering::SeqCst), 0);
    assert!(chain.mutate_state(|state| state.submissions.is_empty()));
    executor.client().close().await;
}

#[tokio::test]
async fn registered_keys_are_skipped_without_submission() {
    let wallet = wallet();
    let payer = payer();
    let (executor, chain) = fixture(&wallet, &payer, 0, true).await;
    let result = executor
        .register_wallet(&wallet, &payer, &WalletRegistrationOptions::default())
        .await
        .unwrap();
    assert_eq!(
        result.accounts,
        vec![AuthoritySigner::account(&wallet.cold)]
    );
    assert_eq!(result.deposit, Balance::from_rao(0));
    assert!(result.result.is_none() && result.finalized_block_hash.is_some());
    assert!(chain.mutate_state(|state| state.submissions.is_empty()));
    executor.client().close().await;
}
