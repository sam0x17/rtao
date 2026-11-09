// Derived from RaoFoundation/subtensor sdk/python/bittensor/hashed.py @ b6a81d144078eac18bd25c3af7cb31579bb670f3 (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/registration_sponsorship.py @ b6a81d144078eac18bd25c3af7cb31579bb670f3 (Apache-2.0); modified by rtao.
//! Hashed recipients in writes: registration guards and sponsorship consent (D-148).
//!
//! The fake chain supplies `HashedAccounts` records, the switch and the
//! reserve; these tests check the composed calls, their metadata and the
//! refusals, not runtime registration semantics.
#![cfg(all(feature = "transfer", feature = "batch"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use anyhow::Result;
use rtao_client::{Client, ClientConfig, Network};
use rtao_core::{
    DynValue,
    codec::Value as CoreValue,
    keys::{CRYPTO_HASHED, CRYPTO_MLDSA65_FIXED, Keypair, encode_hashed_receiving_address},
};
use rtao_intents::AnyIntent;
use rtao_primitives::{
    AccountId, Balance, Tao, TaoBalance,
    receiving::{descriptor_account, descriptor_value, registration_descriptor_value},
};
use rtao_runtime::{
    CallBytes, RuntimeHandle,
    calls::{balances, hashed_accounts, utility},
};
use rtao_signer::{
    BoxFuture, Coldkey, DynKeySource, KeySource, KeyView, Signer, SignerError, SignerRole,
};
use rtao_testkit::{FakeChain, FakeState, rpc::RpcClient};
use rtao_tx::{ExecuteOptions, Executor, PlanOptions, SponsorshipConsent, TxError};
use serde_json::{Value, json};

const COLD: AccountId = AccountId([1; 32]);
const HOT: AccountId = AccountId([2; 32]);
const TAO: u64 = 1_000_000_000;
const FEE: u64 = 73;
const DEPOSIT: u64 = 2 * TAO;

#[derive(Default)]
struct PublicKeys {
    acquisitions: AtomicUsize,
}
impl KeyView for PublicKeys {
    fn hotkey_address(&self) -> Result<AccountId, SignerError> {
        Ok(HOT)
    }
    fn hotkey_public_key(&self) -> Result<[u8; 32], SignerError> {
        Ok(HOT.0)
    }
    fn coldkey_address(&self) -> Result<AccountId, SignerError> {
        Ok(COLD)
    }
}
impl KeySource<Coldkey> for PublicKeys {
    fn address(&self) -> Result<AccountId, SignerError> {
        Ok(COLD)
    }
    async fn signer(&self) -> Result<Arc<dyn Signer>, SignerError> {
        self.acquisitions.fetch_add(1, Ordering::SeqCst);
        Err(SignerError::KeyUnavailable(SignerRole::Coldkey))
    }
}
impl DynKeySource for PublicKeys {
    fn signer_for<'a>(
        &'a self,
        role: SignerRole,
    ) -> BoxFuture<'a, Result<Arc<dyn Signer>, SignerError>> {
        Box::pin(async move {
            self.acquisitions.fetch_add(1, Ordering::SeqCst);
            Err(SignerError::KeyUnavailable(role))
        })
    }
}

fn hashed() -> (Vec<u8>, String, AccountId) {
    let key = Keypair::from_seed(&[9; 32], CRYPTO_HASHED).unwrap();
    let descriptor = key.hashed_descriptor().unwrap();
    let address = encode_hashed_receiving_address(&descriptor).unwrap();
    let account = descriptor_account(&descriptor).unwrap();
    (descriptor, address, account)
}

/// Hashed accounts enabled, the reserve configurable at `DEPOSIT` and, when
/// `registered`, the recipient registered with `descriptor`.
async fn fixture(
    registered: Option<(&[u8], AccountId)>,
    enabled: bool,
) -> Result<(Executor, FakeChain, PublicKeys)> {
    let mut state = FakeState::try_default()?;
    state.fee = FEE;
    state.set_nonce(COLD.to_string(), 4);
    state.seed_python("AdminUtils", "HashedAccountsEnabled", &[], json!(enabled))?;
    state.seed_python(
        "HashedAccounts",
        "HashedAccountRegistrationDeposit",
        &[],
        json!(DEPOSIT),
    )?;
    state.seed_python(
        "System",
        "Account",
        &[json!(COLD.to_string())],
        json!({"data": {"free": 100 * TAO}}),
    )?;
    if let Some((descriptor, account)) = registered {
        state.seed_python(
            "HashedAccounts",
            "Accounts",
            &[json!(account.to_string())],
            json!({
                "descriptor": descriptor_value(descriptor)?,
                "generation": 0,
                "commitment": format!("0x{}", hex::encode(&descriptor[2..])),
            }),
        )?;
    }
    let chain = FakeChain::from_state(state);
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    let client = Client::from_rpc(RpcClient::new(chain.clone()), config).await?;
    Ok((rtao_intents::executor(client), chain, PublicKeys::default()))
}

fn descriptor_arg(descriptor: &[u8]) -> DynValue {
    fn core(value: &Value) -> CoreValue {
        match value {
            Value::String(text) => CoreValue::Str(text.clone()),
            Value::Number(number) => CoreValue::Int(i128::from(number.as_i64().unwrap())),
            Value::Object(fields) => CoreValue::record(
                fields
                    .iter()
                    .map(|(name, value)| (name.clone(), core(value)))
                    .collect(),
            ),
            other => panic!("{other}"),
        }
    }
    DynValue::new(core(&registration_descriptor_value(descriptor).unwrap()))
}

fn transfer_call(runtime: &RuntimeHandle, account: AccountId, rao: u64) -> CallBytes {
    balances::transfer_keep_alive(
        DynValue::new(CoreValue::Bytes(account.0.to_vec())),
        TaoBalance(rao),
    )
    .compose(runtime)
    .unwrap()
}

fn transfer(dest: &str, amount: &str) -> AnyIntent {
    AnyIntent::from_tool("transfer", json!({"dest_ss58": dest, "amount_tao": amount})).unwrap()
}

#[tokio::test]
async fn a_first_payment_registers_the_recipient_in_one_batch() -> Result<()> {
    let (descriptor, address, account) = hashed();
    let (executor, _chain, keys) = fixture(None, true).await?;
    let runtime = executor.client().refresh_runtime().await?;
    let intent = transfer(&address, "1");
    // The JSON keeps the receiving address as written.
    assert_eq!(intent.to_json()["dest_ss58"], json!(address));
    let (plan, preview) = executor
        .plan_and_preflight_dyn(intent.as_dyn(), &keys, &PlanOptions::default())
        .await?;
    let expected = utility::batch_all(vec![
        hashed_accounts::check_registration_deposit(TaoBalance(DEPOSIT)).compose(&runtime)?,
        hashed_accounts::register(descriptor_arg(&descriptor)).compose(&runtime)?,
        transfer_call(&runtime, account, TAO),
    ])
    .compose(&runtime)?;
    assert_eq!(plan.call(), &expected);
    assert_eq!(
        plan.extras["hashed_registration"],
        json!(account.to_string())
    );
    assert_eq!(
        plan.extras["hashed_registration_deposit_rao"],
        json!(DEPOSIT)
    );
    assert_eq!(
        plan.extras["hashed_registration_max_deposit_rao"],
        json!(DEPOSIT)
    );
    assert_eq!(
        plan.extras["hashed_registration_guards"],
        json!([account.to_string()])
    );
    // The reserve counts as spend and is previewed; nothing authorizes it yet.
    assert_eq!(
        plan.spend,
        rtao_primitives::Spend::Bounded(Balance::<Tao>::from_rao(TAO + DEPOSIT))
    );
    let quote = preview.registration_sponsorship.clone().unwrap();
    assert_eq!(
        (quote.recipient_account, quote.payer_account, quote.deposit),
        (account, COLD, Balance::from_rao(DEPOSIT))
    );
    assert!(preview.account_registration);
    assert!(
        preview.effects[0].starts_with("set up recipient and permanently reserve"),
        "{:?}",
        preview.effects
    );
    assert!(
        preview
            .facts
            .iter()
            .any(|(name, _)| name == "Account setup reserve")
    );
    assert_eq!(
        preview.required_free.map(|free| free.rao()),
        Some(TAO + DEPOSIT + FEE + runtime_existential_deposit(&executor).await?)
    );
    assert_eq!(keys.acquisitions.load(Ordering::SeqCst), 0);
    executor.client().close().await;
    Ok(())
}

async fn runtime_existential_deposit(executor: &Executor) -> Result<u64> {
    use rtao_client::ViewExt;
    let view = executor.client().at(rtao_client::BlockRef::Best).await?;
    Ok(view
        .constant(&rtao_runtime::constants::balances::EXISTENTIAL_DEPOSIT)
        .await?
        .0)
}

#[tokio::test]
async fn execution_needs_consent_before_any_key_is_unlocked() -> Result<()> {
    let (_, address, _) = hashed();
    let (executor, chain, keys) = fixture(None, true).await?;
    let intent = transfer(&address, "1");
    let run = |opts: ExecuteOptions| {
        let executor = &executor;
        let keys = &keys;
        let intent = &intent;
        async move { executor.execute_dyn(intent.as_dyn(), keys, &opts).await }
    };
    let reason = |result: Result<rtao_tx::ExtrinsicResult, TxError>| match result {
        Err(TxError::RegistrationSponsorship(error)) => error.reason,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        reason(run(ExecuteOptions::default()).await),
        "registration sponsorship requires consent"
    );
    assert_eq!(
        reason(
            run(ExecuteOptions {
                max_registration_deposit: Some(Balance::from_rao(DEPOSIT - 1)),
                on_registration_sponsorship: Some(SponsorshipConsent::new(|_| async { true })),
                ..Default::default()
            })
            .await
        ),
        "registration deposit exceeds the allowed maximum"
    );
    assert_eq!(
        reason(
            run(ExecuteOptions {
                on_registration_sponsorship: Some(SponsorshipConsent::new(|_| async { false })),
                ..Default::default()
            })
            .await
        ),
        "registration sponsorship was declined"
    );
    assert_eq!(keys.acquisitions.load(Ordering::SeqCst), 0);
    // An allowance that covers the reserve proceeds to the signer.
    let allowed = run(ExecuteOptions {
        max_registration_deposit: Some(Balance::from_rao(DEPOSIT)),
        ..Default::default()
    })
    .await;
    assert!(matches!(allowed, Err(TxError::Signer(_))), "{allowed:?}");
    assert_eq!(keys.acquisitions.load(Ordering::SeqCst), 1);
    assert!(chain.mutate_state(|state| state.submissions.is_empty()));
    executor.client().close().await;
    Ok(())
}

#[tokio::test]
async fn a_registered_recipient_is_checked_and_reserves_nothing() -> Result<()> {
    let (descriptor, address, account) = hashed();
    let (executor, _chain, keys) = fixture(Some((&descriptor, account)), true).await?;
    let runtime = executor.client().refresh_runtime().await?;
    let (plan, preview) = executor
        .plan_and_preflight_dyn(
            transfer(&address, "1").as_dyn(),
            &keys,
            &PlanOptions::default(),
        )
        .await?;
    let expected = utility::batch_all(vec![
        hashed_accounts::check_registered(descriptor_arg(&descriptor)).compose(&runtime)?,
        transfer_call(&runtime, account, TAO),
    ])
    .compose(&runtime)?;
    assert_eq!(plan.call(), &expected);
    assert!(!plan.extras.contains_key("hashed_registration"));
    assert_eq!(plan.extras["hashed_registration_max_deposit_rao"], json!(0));
    assert_eq!(preview.registration_sponsorship, None);
    // A plain SS58 payment to the same account with its explicit descriptor
    // gets the same guard.
    let explicit = AnyIntent::from_tool(
        "transfer",
        json!({
            "dest_ss58": account.to_string(),
            "amount_tao": "1",
            "hashed_descriptor": format!("0x{}", hex::encode(&descriptor)),
        }),
    )?;
    let plan = executor
        .plan_dyn(explicit.as_dyn(), &keys, &PlanOptions::default())
        .await?;
    assert_eq!(plan.call(), &expected);
    executor.client().close().await;
    Ok(())
}

#[tokio::test]
async fn refusals_fail_closed() -> Result<()> {
    let (descriptor, address, account) = hashed();
    // Disabled hashed accounts refuse a new recipient.
    let (executor, _chain, keys) = fixture(None, false).await?;
    let error = executor
        .plan_dyn(
            transfer(&address, "1").as_dyn(),
            &keys,
            &PlanOptions::default(),
        )
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("hashed accounts are not enabled"),
        "{error}"
    );
    executor.client().close().await;

    let (executor, _chain, keys) = fixture(None, true).await?;
    // A descriptor naming another account is refused.
    let mismatch = AnyIntent::from_tool(
        "transfer",
        json!({
            "dest_ss58": COLD.to_string(),
            "amount_tao": "1",
            "hashed_descriptor": format!("0x{}", hex::encode(&descriptor)),
        }),
    )?;
    let error = executor
        .plan_dyn(mismatch.as_dyn(), &keys, &PlanOptions::default())
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("hashed descriptor does not match the destination address"),
        "{error}"
    );
    // A first payment inside a batch is refused; batches only check.
    let batch = AnyIntent::from_tool(
        "batch",
        json!({"intents": [{"op": "transfer", "dest_ss58": address, "amount_tao": "1"}]}),
    )?;
    let error = executor
        .plan_dyn(batch.as_dyn(), &keys, &PlanOptions::default())
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("batched first-time account setup is not supported"),
        "{error}"
    );
    // Proxy dispatch cannot sponsor a first registration.
    let error = executor
        .plan_dyn(
            transfer(&address, "1").as_dyn(),
            &keys,
            &PlanOptions {
                proxy_for: Some(AccountId([7; 32])),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("initial registration cannot be wrapped in proxy, multisig or sudo"),
        "{error}"
    );
    // A build outside the executor never resolves the address unguarded.
    let snapshot = executor.client().at(rtao_client::BlockRef::Best).await?;
    let intent = transfer(&address, "1");
    let error = intent
        .as_dyn()
        .build_dyn(&rtao_tx::BuildCx {
            view: &snapshot as &dyn rtao_client::DynView,
            origin: rtao_tx::OriginView::Keys(&keys),
            runtime: &snapshot.block().runtime,
            clock: &rtao_tx::SystemClock,
            spec: intent.spec(),
        })
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("adds its registration guard"),
        "{error}"
    );
    let _ = account;
    executor.client().close().await;
    Ok(())
}

#[tokio::test]
async fn a_registered_recipient_in_a_batch_is_guarded_inline() -> Result<()> {
    let (descriptor, address, account) = hashed();
    let (executor, _chain, keys) = fixture(Some((&descriptor, account)), true).await?;
    let runtime = executor.client().refresh_runtime().await?;
    let batch = AnyIntent::from_tool(
        "batch",
        json!({"intents": [
            {"op": "transfer", "dest_ss58": address, "amount_tao": "1"},
            {"op": "transfer", "dest_ss58": HOT.to_string(), "amount_tao": "2"},
        ]}),
    )?;
    let plan = executor
        .plan_dyn(batch.as_dyn(), &keys, &PlanOptions::default())
        .await?;
    let expected = utility::batch_all(vec![
        hashed_accounts::check_registered(descriptor_arg(&descriptor)).compose(&runtime)?,
        transfer_call(&runtime, account, TAO),
        transfer_call(&runtime, HOT, 2 * TAO),
    ])
    .compose(&runtime)?;
    assert_eq!(plan.call(), &expected);
    assert_eq!(
        plan.extras["hashed_registration_guards"],
        json!([account.to_string()])
    );
    executor.client().close().await;
    Ok(())
}

#[tokio::test]
async fn a_standard_post_quantum_recipient_needs_no_guard() -> Result<()> {
    let key = Keypair::from_seed(&[5; 32], CRYPTO_MLDSA65_FIXED)?;
    let descriptor = key.authorization_descriptor()?;
    let address = encode_hashed_receiving_address(&descriptor)?;
    let account = descriptor_account(&descriptor)?;
    let (executor, _chain, keys) = fixture(None, true).await?;
    let runtime = executor.client().refresh_runtime().await?;
    let result = executor
        .plan_dyn(
            transfer(&address, "1").as_dyn(),
            &keys,
            &PlanOptions::default(),
        )
        .await;
    if runtime.core().supports_mldsa_profile(descriptor[1]) {
        let plan = result?;
        assert_eq!(plan.call(), &transfer_call(&runtime, account, TAO));
        assert!(!plan.extras.contains_key("hashed_registration_guards"));
    } else {
        let error = result.unwrap_err();
        assert!(
            error
                .to_string()
                .contains("hashed receiving addresses are not supported on this chain"),
            "{error}"
        );
    }
    executor.client().close().await;
    Ok(())
}

// Python v488 `MultisigIntentAdapter.preflight` (D-160): a payment approved
// through a multisig previews its recipient as the receiving address written,
// not the bare account the guarded call pays.
#[cfg(feature = "multisig")]
#[tokio::test]
async fn multisig_previews_name_the_receiving_address_of_a_guarded_payment() -> Result<()> {
    let (descriptor, address, account) = hashed();
    let (executor, _chain, keys) = fixture(Some((&descriptor, account)), true).await?;
    let group = executor.multisig(&[AccountId([42; 32]), COLD], 2).await?;
    let transfer: rtao_intents::Transfer =
        serde_json::from_value(json!({"dest_ss58": address, "amount_tao": "1"}))?;
    let adapter = rtao_intents::MultisigAdapter::new(
        transfer,
        executor.client(),
        group,
        COLD,
        rtao_primitives::CryptoType::Sr25519,
        None,
    )
    .await?;
    let (plan, preview) = executor
        .plan_and_preflight_dyn(&adapter, &keys, &PlanOptions::default())
        .await?;
    let effects = [plan.effects.clone(), preview.effects.clone()].concat();
    assert!(
        effects.iter().any(|effect| effect.contains(&address)),
        "{effects:?}"
    );
    let ss58 = account.to_string();
    assert!(
        effects.iter().all(|effect| !effect.contains(&ss58)),
        "{effects:?}"
    );
    executor.client().close().await;
    Ok(())
}

// Python v488 `test_child_set_submits_guarded_hashed_receiving_address`
// (D-160): a registered hashed child named by its receiving address is
// checked in the same batch, and set_children takes its account.
#[cfg(feature = "children")]
#[tokio::test]
async fn set_children_checks_a_receiving_child_and_assigns_its_account() -> Result<()> {
    let (descriptor, address, account) = hashed();
    let (executor, _chain, keys) = fixture(Some((&descriptor, account)), true).await?;
    let runtime = executor.client().refresh_runtime().await?;
    let intent = AnyIntent::from_tool(
        "set_children",
        json!({"hotkey_ss58": HOT.to_string(), "netuid": 1, "children": [[0.5, address]]}),
    )?;
    let plan = executor
        .plan_dyn(intent.as_dyn(), &keys, &PlanOptions::default())
        .await?;
    let decoded = rtao_core::DynValue::new(runtime.core().decode_spec(
        &rtao_core::runtime::TypeSpec::Call,
        plan.call().as_bytes(),
        true,
    )?)
    .to_json();
    assert_eq!(decoded["call_function"], "batch_all", "{decoded}");
    let calls = decoded["call_args"][0]["value"].as_array().unwrap();
    assert_eq!(calls.len(), 2, "{decoded}");
    assert_eq!(calls[0]["call_function"], "check_registered");
    assert_eq!(calls[1]["call_function"], "set_children");
    let children = calls[1]["call_args"]
        .as_array()
        .unwrap()
        .iter()
        .find(|arg| arg["name"] == "children")
        .unwrap()["value"]
        .clone();
    assert_eq!(
        children,
        json!([[(0.5 * u64::MAX as f64).round() as u64, account.to_string()]])
    );
    executor.client().close().await;
    Ok(())
}
