// Derived from RaoFoundation/subtensor sdk/bittensor-core/tests/e2e.rs @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Public planning and policy counterparts of five pinned upstream scenarios.
//!
//! The production executor consumes metadata-backed FakeChain responses. These
//! tests prove client policy and fee decoding, not runtime fee calibration or
//! the execution of a transfer, stake movement, registration or atomic batch.
#![cfg(all(
    feature = "transfer",
    feature = "batch",
    feature = "staking",
    feature = "registration"
))]

use std::{
    collections::BTreeSet,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result, bail};
use rtao_client::{Client, ClientConfig, Network};
use rtao_core::{codec::Value as CoreValue, runtime::TypeSpec, value_ext::ValueExt};
use rtao_intents::{AnyIntent, Batch, Transfer};
use rtao_primitives::{AccountId, AlphaBalance, Balance, NetUid, Spend, Tao, TaoBalance};
use rtao_runtime::{
    CallBytes,
    calls::{balances, subtensor_module as calls, utility},
    storage::subtensor_module as st,
};
use rtao_signer::{
    BoxFuture, Coldkey, DynKeySource, KeySource, KeyView, Signer, SignerError, SignerRole,
};
use rtao_testkit::{FakeChain, FakeState, rpc::RpcClient};
use rtao_tx::{ExecuteOptions, Executor, PlanOptions, Policy, TxError};
use serde_json::json;

const COLD: AccountId = AccountId([1; 32]);
const HOT: AccountId = AccountId([2; 32]);
const BOB_COLD: AccountId = AccountId([3; 32]);
const BOB_HOT: AccountId = AccountId([4; 32]);
const DAVE: AccountId = AccountId([5; 32]);
const EVE: AccountId = AccountId([6; 32]);
const TAO: u64 = 1_000_000_000;
const FEE: u64 = 73;
const CHAIN_NONCE: u64 = 9;
const CACHED_NONCE: u64 = 40;

// There is no private key or signer in this fixture. Any acquisition attempt
// increments a sentinel and fails instead of silently signing a fake success.
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

// Same native map shape used by the complete catalog plan fixture. No runtime
// accounting is implemented: this is a fixed available-position response.
fn seed_transfer_stake_preview(state: &mut FakeState) -> Result<()> {
    state.seed_runtime(
        "StakeInfoRuntimeApi",
        "get_stake_info_for_hotkey_coldkey_netuid",
        json!({"coldkey":COLD,"hotkey":BOB_HOT,"netuid":1,"stake":10 * TAO}),
    )?;
    let api = "StakeInfoRuntimeApi";
    let method = "get_stake_availability_for_coldkeys";
    let signature = rtao_core::ops::runtime_api::runtime_api_method(&state.runtime, api, method)?;
    let output = CoreValue::Dict(vec![(
        CoreValue::Str(COLD.to_string()),
        CoreValue::Dict(vec![(
            CoreValue::Int(1),
            CoreValue::record(vec![
                ("total".into(), CoreValue::Int(i128::from(10 * TAO))),
                ("locked".into(), CoreValue::Int(0)),
                ("available".into(), CoreValue::Int(i128::from(10 * TAO))),
            ]),
        )]),
    )]);
    let output = state
        .runtime
        .encode_spec(&TypeSpec::Id(signature.output), &output)?;
    let input = rtao_core::ops::runtime_api::encode_params(
        &state.runtime,
        api,
        signature,
        &[
            CoreValue::List(vec![CoreValue::Bytes(COLD.0.to_vec())]),
            CoreValue::List(vec![CoreValue::Int(1)]),
        ],
    )?;
    state.seed_runtime_encoded(
        "StakeInfoRuntimeApi_get_stake_availability_for_coldkeys",
        input,
        output,
    );
    Ok(())
}

async fn fixture() -> Result<(Executor, FakeChain, PublicKeys)> {
    let mut state = FakeState::try_default()?;
    state.fee = FEE;
    state.set_nonce(COLD.to_string(), CHAIN_NONCE);
    state.seed(&st::BURN, (NetUid(1),), TaoBalance(TAO))?;
    state.seed(&st::COLLATERAL_LOCK_SHARE, (NetUid(1),), 0u16)?;
    seed_transfer_stake_preview(&mut state)?;
    let chain = FakeChain::from_state(state);
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    let client = Client::from_rpc(RpcClient::new(chain.clone()), config).await?;
    client.nonces().pin(&COLD, CACHED_NONCE);
    Ok((rtao_intents::executor(client), chain, PublicKeys::default()))
}

fn transfer(destination: AccountId, rao: u64) -> Transfer {
    Transfer {
        dest_ss58: destination.into(),
        amount_tao: Balance::<Tao>::from_rao(rao).into(),
        keep_alive: true,
    }
}

async fn expected_transfer(
    executor: &Executor,
    destination: AccountId,
    rao: u64,
) -> Result<CallBytes> {
    // Construct the expected native call independently from the production
    // intent's build/plan path and its serialized arguments.
    let runtime = executor.client().refresh_runtime().await?;
    Ok(balances::transfer_keep_alive(
        rtao_core::DynValue::new(CoreValue::Bytes(destination.0.to_vec())),
        TaoBalance(rao),
    )
    .compose(&runtime)?)
}

async fn assert_no_private_or_submit(executor: &Executor, chain: &FakeChain, keys: &PublicKeys) {
    assert_eq!(keys.acquisitions.load(Ordering::SeqCst), 0);
    assert!(chain.mutate_state(|state| state.submissions.is_empty()));
    assert!(
        !chain
            .requests()
            .iter()
            .any(|request| request.method.starts_with("author_"))
    );
    assert_eq!(
        executor.client().nonces().cached(&COLD.to_string()).await,
        Some(CACHED_NONCE)
    );
}

// The API input is a complete zero-signature probe plus a little-endian u32
// length. Check the actual request, rather than accepting any seeded fee.
async fn assert_fee_probe(
    executor: &Executor,
    chain: &FakeChain,
    since: usize,
    expected: &CallBytes,
) -> Result<()> {
    let requests = chain.requests();
    let fees = requests[since..]
        .iter()
        .filter(|request| {
            request.method == "state_call"
                && request.params[0] == "TransactionPaymentApi_query_info"
        })
        .collect::<Vec<_>>();
    assert_eq!(fees.len(), 1);
    assert!(fees[0].params[2].is_null());
    let encoded = fees[0].params[1].as_str().context("fee input")?;
    let input = hex::decode(encoded.strip_prefix("0x").context("hex fee input")?)?;
    let frame = input.len().checked_sub(4).context("fee input trailer")?;
    let (extrinsic, length) = input.split_at(frame);
    assert_eq!(
        u32::from_le_bytes(length.try_into()?),
        u32::try_from(extrinsic.len())?
    );
    let runtime = executor.client().refresh_runtime().await?;
    let decoded = runtime.core().decode_extrinsic(extrinsic, true)?;
    let call = runtime
        .core()
        .decode_spec(&TypeSpec::Call, expected.as_bytes(), true)?;
    assert_eq!(decoded.field("call"), Some(&call));
    let account = decoded
        .field("address")
        .and_then(ValueExt::as_str)
        .context("probe address")?;
    assert_eq!(account.parse::<AccountId>()?, COLD);
    assert_eq!(
        decoded.field("nonce").and_then(ValueExt::as_u128),
        Some(u128::from(CHAIN_NONCE))
    );
    assert_eq!(
        decoded
            .field("signature")
            .and_then(|signature| signature.variant("Sr25519"))
            .and_then(ValueExt::value_bytes),
        Some(vec![0u8; 64])
    );
    assert!(
        requests[since..]
            .iter()
            .any(|request| request.method == "account_nextIndex"
                && request.params == json!([COLD.to_string()]))
    );
    Ok(())
}

fn assert_policy(error: TxError, expected: &[&str]) -> Result<()> {
    let TxError::Policy(policy) = error else {
        bail!("expected policy refusal, got {error:?}")
    };
    assert_eq!(policy.violations, expected);
    Ok(())
}

// upstream e2e.rs::test_plan_simulates_real_fee: transfer exactly 1 TAO.
#[tokio::test]
async fn client_transfer_plan_decodes_positive_fee_for_the_exact_call() -> Result<()> {
    let (executor, chain, keys) = fixture().await?;
    let expected = expected_transfer(&executor, BOB_COLD, TAO).await?;
    let start = chain.requests().len();
    let plan = executor
        .plan(&transfer(BOB_COLD, TAO), &keys, &PlanOptions::default())
        .await?;
    assert!(plan.ok());
    assert_eq!(plan.call(), &expected);
    assert_eq!(plan.signer, SignerRole::Coldkey);
    assert_eq!(plan.signer_address, COLD);
    assert_eq!(plan.fee.map(|fee| fee.rao()), Some(FEE));
    assert!(plan.fee.is_some_and(|fee| fee.rao() > 0));
    assert_fee_probe(&executor, &chain, start, &expected).await?;
    assert_no_private_or_submit(&executor, &chain, &keys).await;
    executor.client().close().await;
    Ok(())
}

// upstream e2e.rs::test_policy_blocks_with_live_fee: 5 TAO exceeds 1 TAO cap.
#[tokio::test]
async fn client_transfer_spend_refusal_keeps_fee_probe_public() -> Result<()> {
    let (executor, chain, keys) = fixture().await?;
    let expected = expected_transfer(&executor, BOB_COLD, 5 * TAO).await?;
    let policy = Policy {
        max_spend: Some(Balance::from_rao(TAO)),
        ..Default::default()
    };
    let start = chain.requests().len();
    let plan = executor
        .plan(
            &transfer(BOB_COLD, 5 * TAO),
            &keys,
            &PlanOptions {
                policy: Some(policy.clone()),
                deadline: Some(Duration::from_secs(5)),
                ..Default::default()
            },
        )
        .await?;
    assert_eq!(plan.call(), &expected);
    assert_eq!(plan.fee.map(|fee| fee.rao()), Some(FEE));
    assert_eq!(
        plan.violations,
        ["spend τ5.000000000 exceeds max_spend_tao τ1.000000000"]
    );
    assert_fee_probe(&executor, &chain, start, &expected).await?;
    let start = chain.requests().len();
    let result = executor
        .execute(
            &transfer(BOB_COLD, 5 * TAO),
            &keys,
            &ExecuteOptions {
                policy: Some(policy),
                deadline: Some(Duration::from_secs(5)),
                ..Default::default()
            },
        )
        .await;
    assert_policy(
        result.err().context("5 TAO must exceed 1 TAO")?,
        &["spend τ5.000000000 exceeds max_spend_tao τ1.000000000"],
    )?;
    assert_fee_probe(&executor, &chain, start, &expected).await?;
    assert_no_private_or_submit(&executor, &chain, &keys).await;
    executor.client().close().await;
    Ok(())
}

// upstream e2e.rs::test_policy_aggregates_spend_across_batch: 600m + 600m > 1b.
#[tokio::test]
async fn client_batch_aggregates_both_transfers_before_spend_refusal() -> Result<()> {
    let (executor, chain, keys) = fixture().await?;
    let batch = Batch::new(vec![
        json!({"op":"transfer","dest_ss58":DAVE,"amount_tao":"0.6"}),
        json!({"op":"transfer","dest_ss58":EVE,"amount_tao":"0.6"}),
    ])?;
    let runtime = executor.client().refresh_runtime().await?;
    let expected = utility::batch_all(vec![
        expected_transfer(&executor, DAVE, 600_000_000).await?,
        expected_transfer(&executor, EVE, 600_000_000).await?,
    ])
    .compose(&runtime)?;
    let policy = Policy {
        max_spend: Some(Balance::from_rao(TAO)),
        ..Default::default()
    };
    let start = chain.requests().len();
    let plan = executor
        .plan_dyn(
            &batch,
            &keys,
            &PlanOptions {
                policy: Some(policy.clone()),
                ..Default::default()
            },
        )
        .await?;
    assert_eq!(plan.call(), &expected);
    assert_eq!(plan.spend, Spend::Bounded(Balance::from_rao(1_200_000_000)));
    assert!(!plan.ok());
    assert_eq!(
        plan.violations,
        ["spend τ1.200000000 exceeds max_spend_tao τ1.000000000"]
    );
    assert_eq!(plan.fee.map(|fee| fee.rao()), Some(FEE));
    assert_fee_probe(&executor, &chain, start, &expected).await?;
    let start = chain.requests().len();
    let error = executor
        .execute_dyn(
            &batch,
            &keys,
            &ExecuteOptions {
                policy: Some(policy),
                ..Default::default()
            },
        )
        .await
        .err()
        .context("aggregate cap")?;
    assert_policy(
        error,
        &["spend τ1.200000000 exceeds max_spend_tao τ1.000000000"],
    )?;
    assert_fee_probe(&executor, &chain, start, &expected).await?;
    assert_no_private_or_submit(&executor, &chain, &keys).await;
    executor.client().close().await;
    Ok(())
}

// upstream e2e.rs::test_netuid_allowlist_blocks_live: 1 → 2 and global scope.
#[tokio::test]
async fn client_subnet_allowlist_refuses_destination_and_global_unstake() -> Result<()> {
    let (executor, chain, keys) = fixture().await?;
    let runtime = executor.client().refresh_runtime().await?;
    let moved = AnyIntent::from_tool(
        "move_stake",
        json!({
            "origin_hotkey_ss58":BOB_HOT,"origin_netuid":1,
            "dest_hotkey_ss58":BOB_HOT,"dest_netuid":2,"amount_alpha":"1",
            "slippage_protection":false
        }),
    )?;
    let global = AnyIntent::from_tool("unstake_all_alpha", json!({"hotkey_ss58":BOB_HOT}))?;
    let policy = Policy {
        allowed_netuids: Some(BTreeSet::from([NetUid(1)])),
        ..Default::default()
    };
    for (intent, expected, violation) in [
        (
            moved,
            calls::move_stake(BOB_HOT, BOB_HOT, NetUid(1), NetUid(2), AlphaBalance(TAO))
                .compose(&runtime)?,
            "netuid 2 not in allowed_netuids [1]",
        ),
        (
            global,
            calls::unstake_all_alpha(BOB_HOT).compose(&runtime)?,
            "unstake_all_alpha acts on all subnets, not just allowed_netuids [1]",
        ),
    ] {
        let start = chain.requests().len();
        let plan = executor
            .plan_dyn(
                intent.as_dyn(),
                &keys,
                &PlanOptions {
                    policy: Some(policy.clone()),
                    ..Default::default()
                },
            )
            .await?;
        assert_eq!(plan.call(), &expected);
        assert!(!plan.ok());
        assert_eq!(plan.violations, [violation]);
        assert_eq!(plan.fee.map(|fee| fee.rao()), Some(FEE));
        assert_fee_probe(&executor, &chain, start, &expected).await?;
        let start = chain.requests().len();
        let error = executor
            .execute_dyn(
                intent.as_dyn(),
                &keys,
                &ExecuteOptions {
                    policy: Some(policy.clone()),
                    ..Default::default()
                },
            )
            .await
            .err()
            .context("subnet allowlist")?;
        assert_policy(error, &[violation])?;
        assert_fee_probe(&executor, &chain, start, &expected).await?;
        assert_no_private_or_submit(&executor, &chain, &keys).await;
    }
    executor.client().close().await;
    Ok(())
}

// upstream e2e.rs::test_spend_cap_blocks_value_movers: all three unbounded ops.
#[tokio::test]
async fn client_spend_cap_refuses_all_three_unbounded_value_movers() -> Result<()> {
    let (executor, chain, keys) = fixture().await?;
    let runtime = executor.client().refresh_runtime().await?;
    let transfer_stake = AnyIntent::from_tool(
        "transfer_stake",
        json!({
            "dest_coldkey_ss58":BOB_COLD,"hotkey_ss58":BOB_HOT,
            "origin_netuid":1,"dest_netuid":1,"amount_alpha":"1"
        }),
    )?;
    let register = AnyIntent::from_tool("register_subnet", json!({"hotkey_ss58":HOT}))?;
    let burned = AnyIntent::from_tool("burned_register", json!({"netuid":1,"hotkey_ss58":HOT}))?;
    let policy = Policy {
        max_spend: Some(Balance::from_rao(TAO)),
        ..Default::default()
    };
    for (intent, expected, operation) in [
        (
            transfer_stake,
            calls::transfer_stake(BOB_COLD, BOB_HOT, NetUid(1), NetUid(1), AlphaBalance(TAO))
                .compose(&runtime)?,
            "transfer_stake",
        ),
        (
            register,
            calls::register_network(HOT).compose(&runtime)?,
            "register_subnet",
        ),
        (
            burned,
            calls::burned_register(NetUid(1), HOT).compose(&runtime)?,
            "burned_register",
        ),
    ] {
        let violation = format!(
            "{operation} spends an amount that cannot be bounded before execution; max_spend_tao τ1.000000000 blocks it"
        );
        let start = chain.requests().len();
        let plan = executor
            .plan_dyn(
                intent.as_dyn(),
                &keys,
                &PlanOptions {
                    policy: Some(policy.clone()),
                    ..Default::default()
                },
            )
            .await?;
        assert_eq!(plan.call(), &expected);
        assert_eq!(plan.spend, Spend::Unbounded);
        assert!(!plan.ok());
        assert_eq!(plan.violations, [violation.as_str()]);
        assert_eq!(plan.fee.map(|fee| fee.rao()), Some(FEE));
        assert_fee_probe(&executor, &chain, start, &expected).await?;
        let start = chain.requests().len();
        let error = executor
            .execute_dyn(
                intent.as_dyn(),
                &keys,
                &ExecuteOptions {
                    policy: Some(policy.clone()),
                    ..Default::default()
                },
            )
            .await
            .err()
            .context("unbounded spend")?;
        assert_policy(error, &[violation.as_str()])?;
        if operation == "burned_register" {
            // Python requires the shield for this intent. Its semantic spend
            // check refuses before either signature or a carrier fee probe.
            assert!(
                !chain.requests()[start..]
                    .iter()
                    .any(|request| request.method == "state_call"
                        && request.params[0] == "TransactionPaymentApi_query_info")
            );
        } else {
            assert_fee_probe(&executor, &chain, start, &expected).await?;
        }
        assert_no_private_or_submit(&executor, &chain, &keys).await;
    }
    executor.client().close().await;
    Ok(())
}
