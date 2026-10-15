// Derived from RaoFoundation/subtensor sdk/python/bittensor/intents/base.py and signing.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Role selection and public-default behavior without private-key unlocks.
#![allow(clippy::expect_used)]

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use futures::executor::block_on;
use rtao_core::keys;
use rtao_signer::{
    AccountId, BoxFuture, Coldkey, CryptoType, DynKeySource, Hotkey, IntentKeyView, KeySource,
    KeyView, KeypairSigner, Role, Signer, SignerError, SignerRole,
};

fn bare() -> KeypairSigner {
    KeypairSigner::from_seed(&[17; 32], CryptoType::Ed25519).expect("test seed")
}

#[test]
fn bare_sources_can_sign_either_role_but_have_no_attached_keys() {
    let source = bare();
    assert_eq!(
        <KeypairSigner as KeySource<Coldkey>>::address(&source).expect("cold address"),
        source.account()
    );
    assert_eq!(
        <KeypairSigner as KeySource<Hotkey>>::address(&source).expect("hot address"),
        source.account()
    );
    assert_eq!(
        source.hotkey_address(),
        Err(SignerError::KeyUnavailable(SignerRole::Hotkey))
    );
    assert_eq!(
        source.hotkey_public_key(),
        Err(SignerError::KeyUnavailable(SignerRole::Hotkey))
    );
    assert_eq!(
        source.coldkey_address(),
        Err(SignerError::KeyUnavailable(SignerRole::Coldkey))
    );
    assert_eq!(source.bare_signer_account(), Some(source.account()));
    let signers = [
        block_on(<KeypairSigner as KeySource<Coldkey>>::signer(&source)).expect("cold signer"),
        block_on(<KeypairSigner as KeySource<Hotkey>>::signer(&source)).expect("hot signer"),
        block_on(source.signer_for(SignerRole::Coldkey)).expect("dynamic cold signer"),
        block_on(source.signer_for(SignerRole::Hotkey)).expect("dynamic hot signer"),
    ];
    drop(source);
    for signer in signers {
        let message = b"both roles use the same bare identity";
        let signature = block_on(signer.sign_raw(message)).expect("signer survives source drop");
        assert!(
            keys::verify_strict(
                message,
                &signature,
                &signer.account().to_string(),
                u8::from(signer.crypto_type())
            )
            .expect("verify")
        );
    }
}

#[test]
fn cross_role_defaults_and_diagnostics_match_python() {
    let source = bare();
    let cold = IntentKeyView::new(&source, SignerRole::Coldkey, "register");
    assert_eq!(
        cold.coldkey_address().expect("own cold identity"),
        source.account()
    );
    assert_eq!(
        cold.hotkey_address(None)
            .expect_err("no default hotkey")
            .to_string(),
        "register: this signer carries no hotkey to default to; pass the hotkey address explicitly or sign with a Wallet"
    );
    assert_eq!(
        cold.hotkey_public_key()
            .expect_err("no default hotkey key")
            .to_string(),
        "register: this signer carries no hotkey public key; sign with a Wallet or a hotkey Signer"
    );
    let explicit = AccountId([23; 32]);
    assert_eq!(
        cold.hotkey_address(Some(explicit)).expect("explicit wins"),
        explicit
    );
    let hot = IntentKeyView::new(&source, SignerRole::Hotkey, "set_weights");
    assert_eq!(
        hot.hotkey_address(None).expect("own hot identity"),
        source.account()
    );
    assert_eq!(
        hot.hotkey_public_key().expect("own public key"),
        source.account().0
    );
    assert_eq!(
        hot.coldkey_address()
            .expect_err("no default coldkey")
            .to_string(),
        "set_weights: this signer carries no coldkey to reference; sign with a Wallet or a coldkey Signer"
    );
}

struct LockedWallet {
    public_calls: AtomicUsize,
    unlocks: AtomicUsize,
    cold: Arc<dyn Signer>,
    hot: Arc<dyn Signer>,
}

impl KeyView for LockedWallet {
    fn hotkey_address(&self) -> Result<AccountId, SignerError> {
        self.public_calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.hot.account())
    }
    fn hotkey_public_key(&self) -> Result<[u8; 32], SignerError> {
        self.public_calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.hot.account().0)
    }
    fn coldkey_address(&self) -> Result<AccountId, SignerError> {
        self.public_calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.cold.account())
    }
}

impl KeySource<Coldkey> for LockedWallet {
    fn address(&self) -> Result<AccountId, SignerError> {
        self.coldkey_address()
    }
    async fn signer(&self) -> Result<Arc<dyn Signer>, SignerError> {
        self.unlocks.fetch_add(1, Ordering::SeqCst);
        Ok(self.cold.clone())
    }
}
impl KeySource<Hotkey> for LockedWallet {
    fn address(&self) -> Result<AccountId, SignerError> {
        self.hotkey_address()
    }
    async fn signer(&self) -> Result<Arc<dyn Signer>, SignerError> {
        self.unlocks.fetch_add(1, Ordering::SeqCst);
        Ok(self.hot.clone())
    }
}
impl DynKeySource for LockedWallet {
    fn signer_for<'a>(
        &'a self,
        role: SignerRole,
    ) -> BoxFuture<'a, Result<Arc<dyn Signer>, SignerError>> {
        Box::pin(async move {
            match role {
                SignerRole::Coldkey => <Self as KeySource<Coldkey>>::signer(self).await,
                SignerRole::Hotkey => <Self as KeySource<Hotkey>>::signer(self).await,
            }
        })
    }
}

fn wallet() -> LockedWallet {
    LockedWallet {
        public_calls: AtomicUsize::new(0),
        unlocks: AtomicUsize::new(0),
        cold: Arc::new(bare()),
        hot: Arc::new(KeypairSigner::from_seed(&[18; 32], CryptoType::Sr25519).expect("test seed")),
    }
}

#[test]
fn wallet_public_views_and_intent_defaults_never_unlock() {
    let source = wallet();
    assert_eq!(source.bare_signer_account(), None);
    for role in [SignerRole::Coldkey, SignerRole::Hotkey] {
        let view = IntentKeyView::new(&source, role, "test");
        assert_eq!(
            view.hotkey_address(None).expect("attached hot"),
            source.hot.account()
        );
        assert_eq!(
            view.hotkey_public_key().expect("attached public key"),
            source.hot.account().0
        );
        assert_eq!(
            view.coldkey_address().expect("attached cold"),
            source.cold.account()
        );
    }
    assert_eq!(
        <LockedWallet as KeySource<Coldkey>>::address(&source).expect("public cold address"),
        source.cold.account()
    );
    assert_eq!(
        <LockedWallet as KeySource<Hotkey>>::address(&source).expect("public hot address"),
        source.hot.account()
    );
    assert_eq!(source.unlocks.load(Ordering::SeqCst), 0);
    assert_eq!(source.public_calls.load(Ordering::SeqCst), 8);
    block_on(source.signer_for(SignerRole::Coldkey)).expect("dynamic cold signer");
    block_on(<LockedWallet as KeySource<Hotkey>>::signer(&source)).expect("typed hot signer");
    assert_eq!(source.unlocks.load(Ordering::SeqCst), 2);
}

#[test]
fn explicit_override_skips_public_io_and_attached_errors_propagate() {
    struct Broken;
    impl KeyView for Broken {
        fn hotkey_address(&self) -> Result<AccountId, SignerError> {
            Err(SignerError::Core(rtao_core::CoreError::Keyfile(
                "unreadable public file".into(),
            )))
        }
        fn hotkey_public_key(&self) -> Result<[u8; 32], SignerError> {
            Err(SignerError::Core(rtao_core::CoreError::Keyfile(
                "unreadable public file".into(),
            )))
        }
        fn coldkey_address(&self) -> Result<AccountId, SignerError> {
            Err(SignerError::Core(rtao_core::CoreError::Keyfile(
                "unreadable public file".into(),
            )))
        }
        fn bare_signer_account(&self) -> Option<AccountId> {
            Some(AccountId([19; 32]))
        }
    }
    let view = IntentKeyView::new(&Broken, SignerRole::Hotkey, "test");
    assert_eq!(
        view.hotkey_address(Some(AccountId([20; 32])))
            .expect("override skips error"),
        AccountId([20; 32])
    );
    for result in [view.hotkey_address(None), view.coldkey_address()] {
        assert!(matches!(result, Err(SignerError::Core(_))));
    }
    assert!(matches!(
        view.hotkey_public_key(),
        Err(SignerError::Core(_))
    ));
}

#[test]
fn arc_dynamic_bare_signer_preserves_capabilities_and_role_semantics() {
    let signer: Arc<dyn Signer> = Arc::new(bare());
    let view: &dyn KeyView = &signer;
    assert_eq!(view.bare_signer_account(), Some(signer.account()));
    let source: &dyn DynKeySource = &signer;
    let resolved = block_on(source.signer_for(SignerRole::Hotkey)).expect("dynamic source");
    assert_eq!(resolved.account(), signer.account());
    assert_eq!(resolved.caps(), signer.caps());
    assert_eq!(resolved.crypto_type(), signer.crypto_type());
    assert_eq!(Hotkey::KIND, SignerRole::Hotkey);
    assert_eq!(Coldkey::KIND, SignerRole::Coldkey);
}

#[test]
fn unknown_roles_fail_typed_parse_before_unlock() {
    for role in ["", "owner", "Coldkey", " HOTKEY "] {
        assert!(
            matches!(role.parse::<SignerRole>(),Err(SignerError::InvalidRole(value)) if value==role)
        );
    }
    assert_eq!(
        SignerRole::try_from("coldkey").expect("known role"),
        SignerRole::Coldkey
    );
    assert_eq!(
        SignerRole::try_from("hotkey").expect("known role"),
        SignerRole::Hotkey
    );
}

#[test]
fn role_specific_dynamic_source_refuses_before_using_its_signer() {
    struct HotOnly {
        signer: Arc<dyn Signer>,
        requests: AtomicUsize,
    }
    impl KeyView for HotOnly {
        fn hotkey_address(&self) -> Result<AccountId, SignerError> {
            Ok(self.signer.account())
        }
        fn hotkey_public_key(&self) -> Result<[u8; 32], SignerError> {
            Ok(self.signer.account().0)
        }
        fn coldkey_address(&self) -> Result<AccountId, SignerError> {
            Err(SignerError::KeyUnavailable(SignerRole::Coldkey))
        }
    }
    impl KeySource<Hotkey> for HotOnly {
        fn address(&self) -> Result<AccountId, SignerError> {
            self.hotkey_address()
        }
        async fn signer(&self) -> Result<Arc<dyn Signer>, SignerError> {
            self.requests.fetch_add(1, Ordering::SeqCst);
            Ok(self.signer.clone())
        }
    }
    impl DynKeySource for HotOnly {
        fn signer_for<'a>(
            &'a self,
            role: SignerRole,
        ) -> BoxFuture<'a, Result<Arc<dyn Signer>, SignerError>> {
            Box::pin(async move {
                match role {
                    SignerRole::Coldkey => Err(SignerError::RoleUnavailable(role)),
                    SignerRole::Hotkey => <Self as KeySource<Hotkey>>::signer(self).await,
                }
            })
        }
    }
    let source = HotOnly {
        signer: Arc::new(bare()),
        requests: AtomicUsize::new(0),
    };
    assert!(matches!(
        block_on(source.signer_for(SignerRole::Coldkey)),
        Err(SignerError::RoleUnavailable(SignerRole::Coldkey))
    ));
    assert_eq!(source.requests.load(Ordering::SeqCst), 0);
    assert_eq!(
        block_on(source.signer_for(SignerRole::Hotkey))
            .expect("allowed hot")
            .account(),
        <HotOnly as KeySource<Hotkey>>::address(&source).expect("public account")
    );
    assert_eq!(source.requests.load(Ordering::SeqCst), 1);
}
