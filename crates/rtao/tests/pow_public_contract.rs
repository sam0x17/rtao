// Derived from RaoFoundation/subtensor sdk/python/tests/unit/test_pow_registration.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Public blocking proof acquisition and native intent planning over a fake chain.

#![cfg(all(feature = "blocking", feature = "pow"))]

use std::{
    fmt::Write,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use rtao::{
    blocking::SyncClient,
    client::{Client, ClientConfig, Network},
    intents::{
        PowRegister,
        pow::{CancellationToken, PowConfig, registration_prefix},
    },
    primitives::{AccountId, NetUid, Spend},
    signer::{Coldkey, KeySource, KeyView, Signer, SignerError},
    tx::PlanOptions,
};
use rtao_testkit::{FakeChain, FakeState, rpc::RpcClient};
use serde_json::json;

const NETUID: NetUid = NetUid(1);
const HOT: AccountId = AccountId([2; 32]);
const COLD: AccountId = AccountId([3; 32]);

struct PublicKeys(AtomicBool);

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
        self.0.store(true, Ordering::SeqCst);
        Err(SignerError::KeyUnavailable(
            rtao::signer::SignerRole::Coldkey,
        ))
    }
}

// pyt: unit/test_pow_registration.py::test_public_miner_returns_fresh_intent_without_a_signing_key
// pyt: unit/test_pow_registration.py::test_sync_sdk_exposes_the_same_public_miner
// Native mining returns PowProof; the caller explicitly constructs PowRegister.
// This fixture uses one CPU worker at difficulty one. It loads no GPU driver,
// owns no wallet, submits no extrinsic and proves no runtime registration effect.
#[test]
fn sync_public_pow_proof_builds_exact_registration_plan_without_private_keys() -> anyhow::Result<()>
{
    let mut state = FakeState::new(FakeState::try_default()?.runtime);
    state.set_finalized(98)?;
    state.seed_typed("SubtensorModule", "NetworksAdded", (NETUID,), true)?;
    state.seed_typed("SubtensorModule", "MaxAllowedUids", (NETUID,), 16_u16)?;
    state.seed_typed(
        "SubtensorModule",
        "NetworkRegistrationAllowed",
        (NETUID,),
        false,
    )?;
    state.seed_typed(
        "SubtensorModule",
        "NetworkPowRegistrationAllowed",
        (NETUID,),
        true,
    )?;
    for (item, value) in [
        ("Difficulty", 1_u64),
        ("MinDifficulty", 1),
        ("MaxDifficulty", u64::MAX),
    ] {
        state.seed_typed("SubtensorModule", item, (NETUID,), value)?;
    }
    state.seed_typed(
        "SubtensorModule",
        "MaxRegistrationsPerBlock",
        (NETUID,),
        1_u16,
    )?;
    state.seed_typed(
        "SubtensorModule",
        "RegistrationsThisBlock",
        (NETUID,),
        0_u16,
    )?;
    let fake = FakeChain::from_state(state);
    let transport = fake.clone();
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
    config.runtime_cache_dir = None;
    config.config_store = None;
    config.track_heads = false;
    let client = SyncClient::with_connector(config, move |config| {
        let transport = transport.clone();
        async move { Client::from_rpc(RpcClient::new(transport), config).await }
    });
    let keys = Arc::new(PublicKeys(AtomicBool::new(false)));
    let outcome = (|| -> anyhow::Result<_> {
        let proof = client.mine_pow_registration(
            NETUID,
            HOT,
            COLD,
            PowConfig {
                workers: 1,
                timeout: Duration::from_secs(10),
            },
            CancellationToken::new(),
        )?;
        let mut work_hex = String::with_capacity(64);
        for byte in proof.work {
            write!(&mut work_hex, "{byte:02x}")?;
        }
        let intent: PowRegister = serde_json::from_value(json!({
            "netuid": NETUID.0,
            "work_block": proof.work_block,
            "nonce": proof.nonce,
            "work_hex": work_hex,
        }))?;
        let plan = client.plan(intent, keys.clone(), PlanOptions::default())?;
        Ok((proof, plan))
    })();
    let closed = client.close();
    let (proof, plan) = outcome?;
    closed?;

    assert_eq!(proof.work_block, 100);
    assert_eq!((proof.hotkey, proof.coldkey), (HOT, COLD));
    let prefix = registration_prefix(NETUID, proof.block_hash, HOT, COLD)?;
    let seal = rtao::core::pow::seal(&prefix, proof.nonce).map_err(anyhow::Error::msg)?;
    assert_eq!(proof.work, seal);
    assert!(plan.ok(), "{plan:?}");
    assert_eq!(plan.op, "pow_register");
    assert_eq!(plan.signer_address, COLD);
    assert_eq!(plan.spend, Spend::None);
    // The pinned v478 runtime's call indices precede its exact SCALE arguments.
    let mut expected = vec![7, 152];
    expected.extend_from_slice(&NETUID.0.to_le_bytes());
    expected.extend_from_slice(&proof.work_block.to_le_bytes());
    expected.extend_from_slice(&proof.nonce.to_le_bytes());
    expected.extend_from_slice(&proof.work);
    expected.extend_from_slice(&HOT.0);
    assert_eq!(plan.call().as_bytes(), expected);
    assert!(!keys.0.load(Ordering::SeqCst));
    assert!(client.is_closed());
    assert!(
        fake.requests()
            .iter()
            .all(|request| !request.method.starts_with("author_"))
    );
    Ok(())
}
