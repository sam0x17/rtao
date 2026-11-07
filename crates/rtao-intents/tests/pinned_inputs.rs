// Derived from RaoFoundation/subtensor sdk/python/bittensor/_pinned.py @ b6a81d144078eac18bd25c3af7cb31579bb670f3 (Apache-2.0); modified by rtao.
//! A later multisig round that rebuilds its call keeps round 1's dynamic inputs.
#![cfg(all(feature = "staking", feature = "children", feature = "batch"))]
#![allow(clippy::unwrap_used)] // Assertions must fail when fake-chain fixtures are invalid.

use anyhow::Result;
use rtao_client::{BlockRef, Client, ClientConfig, Network, Snapshot};
use rtao_intents::AnyIntent;
use rtao_primitives::{AccountId, PerU16};
use rtao_runtime::{CallBytes, storage::subtensor_module as st};
use rtao_signer::{KeyView, SignerError};
use rtao_testkit::{FakeChain, FakeState, rpc::RpcClient};
use rtao_tx::{BuildCx, OriginView, SystemClock, pinned};
use serde_json::{Value, json};

const RAO: u64 = 1_000_000_000;
const COLD: AccountId = AccountId([1; 32]);
const HOT: AccountId = AccountId([2; 32]);

struct Keys;
impl KeyView for Keys {
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

/// A chain where the coldkey holds `free`, netuid 1 trades at `price` and the
/// hotkey's delegate take is `take`.
async fn chain(free: u64, price: u64, take: u16) -> Result<(Client, Snapshot)> {
    let mut state = FakeState::try_default()?;
    state.seed_python(
        "System",
        "Account",
        &[json!(COLD.to_string())],
        json!({"data": {"free": free}}),
    )?;
    state.seed_runtime("SwapRuntimeApi", "current_alpha_price", json!(price))?;
    state.seed(&st::DELEGATES, (HOT,), PerU16(take))?;
    let fake = FakeChain::from_state(state);
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    let client = Client::from_rpc(RpcClient::new(fake), config).await?;
    let snapshot = client.at(BlockRef::Finalized).await?;
    Ok((client, snapshot))
}

async fn build(intent: &AnyIntent, snapshot: &Snapshot) -> Result<CallBytes, rtao_tx::IntentError> {
    Ok(intent
        .as_dyn()
        .build_dyn(&BuildCx {
            view: snapshot,
            origin: OriginView::Keys(&Keys),
            runtime: &snapshot.block().runtime,
            clock: &SystemClock,
            spec: intent.spec(),
        })
        .await?
        .call)
}

async fn rebuild(
    intent: &AnyIntent,
    snapshot: &Snapshot,
    pinned_call: &CallBytes,
) -> Result<CallBytes, rtao_tx::IntentError> {
    let inputs = pinned::PinnedInputs::decode(pinned_call, &snapshot.block().runtime)?;
    pinned::scope(inputs, build(intent, snapshot)).await
}

fn batch(children: Value) -> AnyIntent {
    AnyIntent::from_tool("batch", json!({ "intents": children })).unwrap()
}

#[tokio::test]
async fn rebuilt_all_amounts_prices_and_take_branches_come_from_the_approval() -> Result<()> {
    let intent = batch(json!([
        {"op": "add_stake", "hotkey_ss58": HOT.to_string(), "netuid": 1, "amount_tao": "all"},
        {"op": "set_take", "hotkey_ss58": HOT.to_string(), "take": 5},
        {"op": "remove_stake", "hotkey_ss58": HOT.to_string(), "netuid": 1, "amount_alpha": 1.0},
    ]));
    let (first_client, first) = chain(100 * RAO, 20 * RAO, 10).await?;
    let approved = build(&intent, &first).await?;
    first_client.close().await;

    // Balance, price and current take all moved: a fresh build differs (the
    // take is now below the target, so it would even pick increase_take).
    let (client, later) = chain(50 * RAO, 40 * RAO, 1).await?;
    let fresh = build(&intent, &later).await?;
    assert_ne!(fresh.as_bytes(), approved.as_bytes());
    // Rebuilding with the approval reproduces it exactly.
    let rebuilt = rebuild(&intent, &later, &approved).await?;
    assert_eq!(rebuilt.as_bytes(), approved.as_bytes());

    // Explicit arguments are never taken from the approval: a different
    // explicit amount still differs, and is caught by the byte comparison.
    let changed = batch(json!([
        {"op": "add_stake", "hotkey_ss58": HOT.to_string(), "netuid": 1, "amount_tao": "all"},
        {"op": "set_take", "hotkey_ss58": HOT.to_string(), "take": 5},
        {"op": "remove_stake", "hotkey_ss58": HOT.to_string(), "netuid": 1, "amount_alpha": 2.0},
    ]));
    let rebuilt = rebuild(&changed, &later, &approved).await?;
    assert_ne!(rebuilt.as_bytes(), approved.as_bytes());

    // A staking call that the approval does not contain is refused.
    let other = batch(json!([
        {"op": "add_stake", "hotkey_ss58": HOT.to_string(), "netuid": 1, "amount_tao": "all",
         "slippage_protection": false},
    ]));
    let error = rebuild(&other, &later, &approved).await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "imported multisig call does not match the staking operation"
    );
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn outside_a_rebuild_builds_derive_from_current_state() -> Result<()> {
    let intent = AnyIntent::from_tool(
        "set_take",
        json!({"hotkey_ss58": HOT.to_string(), "take": 5}),
    )?;
    let (client, snapshot) = chain(RAO, RAO, 5).await?;
    let error = build(&intent, &snapshot).await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "delegate take is already 5/65535; nothing to change"
    );
    client.close().await;
    Ok(())
}
