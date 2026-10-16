//! Executable offline examples shared with the generated getting-started guide.

#[docify::export]
#[test]
fn checked_native_balances() -> Result<(), rtao::primitives::UnitError> {
    use rtao::primitives::{Balance, Tao, UnitError};

    let amount = Balance::<Tao>::parse("1.25")?;
    assert_eq!(amount.rao(), 1_250_000_000);

    let one_rao = Balance::<Tao>::from_rao(1);
    let maximum = Balance::<Tao>::from_rao(u64::MAX);
    assert_eq!(maximum.checked_add(one_rao), Err(UnitError::Overflow));
    assert_eq!(
        Balance::<Tao>::ZERO.checked_sub(one_rao),
        Err(UnitError::Underflow)
    );
    Ok(())
}

#[docify::export]
#[test]
fn python_compatible_number_formatting() {
    use rtao::primitives::pyfmt;

    assert_eq!(pyfmt::float_repr(0.00001), "1e-05");
    assert_eq!(pyfmt::float_repr(-0.0), "-0.0");
}

#[cfg(feature = "btauth")]
#[docify::export]
#[test]
fn btauth_payload_matches_committed_vector() -> anyhow::Result<()> {
    use anyhow::Context;
    use rtao::{
        btauth::build_payload,
        primitives::{AccountId, CryptoType, json, pyfmt},
    };
    use serde_json::Value;

    let recorded: Value = json::from_slice(include_bytes!(
        "../../../vendor/upstream/extracted/btauth.json"
    ))?;
    assert_eq!(recorded["format"], 1);
    let values = &recorded["values"];
    let method: http::Method = values["METHOD"]["value"]
        .as_str()
        .context("recorded method")?
        .parse()?;
    let target = values["PATH"]["value"]
        .as_str()
        .context("recorded target")?;
    let nonce = values["FIXED_NONCE"]["value"]
        .as_u64()
        .context("recorded nonce")?;
    let body = pyfmt::bytes_fromhex(
        values["BODY"]["value"]["bytes_hex"]
            .as_str()
            .context("recorded body")?,
    )
    .map_err(anyhow::Error::msg)?;
    let expected = pyfmt::bytes_fromhex(
        values["GOLDEN_SR25519_PAYLOAD"]["value"]["bytes_hex"]
            .as_str()
            .context("recorded payload")?,
    )
    .map_err(anyhow::Error::msg)?;
    let sender: AccountId = "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY".parse()?;
    let receiver: AccountId = "5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty".parse()?;
    let payload = build_payload(
        CryptoType::Sr25519,
        &method,
        target,
        &body,
        nonce,
        &sender,
        Some(&receiver),
    );
    assert_eq!(payload, expected);
    Ok(())
}

#[cfg(feature = "reads")]
#[docify::export]
#[tokio::test]
async fn fakechain_balance_read_stays_on_one_snapshot() -> anyhow::Result<()> {
    use rtao::{client::Network, prelude::*};
    use rtao_testkit::{FakeChain, FakeState, RecordingView, rpc::RpcClient};
    use serde_json::json;
    use std::collections::BTreeSet;

    let account = AccountId([7; 32]);
    let mut state = FakeState::try_default()?;
    state.seed_python(
        "System",
        "Account",
        &[json!(account.to_string())],
        json!({"data": {"free": 1_250_000_000u64}}),
    )?;
    let fake = FakeChain::from_state(state);
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    let client = Client::from_rpc(RpcClient::new(fake.clone()), config).await?;
    let result = async {
        let snapshot = client.at(BlockRef::Finalized).await?;
        let at = snapshot.block().hash;
        let view = RecordingView::new(snapshot);
        let balance = view.balances().balance(account).await?;
        Ok::<_, anyhow::Error>((balance.rao, view.block_hashes(), at))
    }
    .await;
    client.close().await;

    let (rao, observed, at) = result?;
    assert_eq!(rao, 1_250_000_000);
    assert_eq!(observed, BTreeSet::from([at]));
    assert!(
        fake.requests()
            .iter()
            .all(|request| !request.method.starts_with("author_"))
    );
    Ok(())
}

#[cfg(feature = "intents-all")]
#[docify::export]
#[tokio::test]
async fn transfer_plan_uses_public_keys_without_signing() -> anyhow::Result<()> {
    use rtao::{
        Executor,
        client::{Client, ClientConfig, Network},
        intents::Transfer,
        primitives::{AccountId, Balance, Spend, Tao},
        signer::{Coldkey, KeySource, KeyView, Signer, SignerError, SignerRole},
        tx::PlanOptions,
    };
    use rtao_testkit::{FakeChain, FakeState, rpc::RpcClient};
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    struct PublicColdkey(AccountId, AtomicBool);
    impl KeyView for PublicColdkey {
        fn hotkey_address(&self) -> Result<AccountId, SignerError> {
            Err(SignerError::KeyUnavailable(SignerRole::Hotkey))
        }
        fn hotkey_public_key(&self) -> Result<[u8; 32], SignerError> {
            Err(SignerError::KeyUnavailable(SignerRole::Hotkey))
        }
        fn coldkey_address(&self) -> Result<AccountId, SignerError> {
            Ok(self.0)
        }
    }
    impl KeySource<Coldkey> for PublicColdkey {
        fn address(&self) -> Result<AccountId, SignerError> {
            Ok(self.0)
        }
        async fn signer(&self) -> Result<Arc<dyn Signer>, SignerError> {
            self.1.store(true, Ordering::SeqCst);
            Err(SignerError::KeyUnavailable(SignerRole::Coldkey))
        }
    }

    let sender = AccountId([7; 32]);
    let keys = PublicColdkey(sender, AtomicBool::new(false));
    let mut state = FakeState::try_default()?;
    state.fee = 12;
    let fake = FakeChain::from_state(state);
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    let client = Client::from_rpc(RpcClient::new(fake.clone()), config).await?;
    client.nonces().pin(&sender, 40);
    let amount = Balance::<Tao>::from_rao(1);
    let intent = Transfer {
        dest_ss58: AccountId([9; 32]).into(),
        amount_tao: amount.into(),
        keep_alive: true,
    };
    let result = Executor::new(client.clone())
        .plan(&intent, &keys, &PlanOptions::default())
        .await;
    let nonce = client.nonces().cached(&sender.to_string()).await;
    client.close().await;

    let plan = result?;
    assert!(plan.ok());
    assert_eq!(plan.op, "transfer");
    assert_eq!(plan.signer_address, sender);
    assert_eq!(plan.spend, Spend::Bounded(amount));
    assert_eq!(plan.fee.map(Balance::rao), Some(12));
    assert!(!keys.1.load(Ordering::SeqCst));
    assert_eq!(nonce, Some(40));
    assert!(
        fake.requests()
            .iter()
            .all(|request| !request.method.starts_with("author_"))
    );
    Ok(())
}
