// Derived from RaoFoundation/subtensor sdk/python/bittensor/intents/gamma.py @ b6a81d144078eac18bd25c3af7cb31579bb670f3 (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/tests/unit/test_gamma.py @ b6a81d144078eac18bd25c3af7cb31579bb670f3 (Apache-2.0); modified by rtao.
//! Gamma intents: chain units, quotes, deposit previews and input checks.
#![cfg(feature = "gamma")]
#![allow(clippy::unwrap_used)] // Assertions must fail when fake-chain fixtures are invalid.

use anyhow::Result;
use rtao_client::{BlockRef, Client, ClientConfig, Network, Snapshot};
use rtao_core::{DynValue, codec::Value as CoreValue};
use rtao_intents::{
    AnyIntent, CloseGammaDeposit, MintGamma, MintGammaWithStake, MoveGamma,
    SetGammaDepositDailySpendCap, SetGammaDepositSpender,
};
use rtao_primitives::{AccountId, AlphaBalance, NetUid, Spend, TaoBalance};
use rtao_runtime::calls::subtensor_module as calls;
use rtao_signer::{KeyView, SignerError};
use rtao_testkit::{
    FakeChain, FakeState,
    fixtures::catalog_samples::{COLD, HOT, OTHER_HOT, intent_state},
    rpc::RpcClient,
};
use rtao_tx::{BuildCx, Intent, IntentError, OriginView, SystemClock};
use serde_json::{Value, json};

const RAO: u64 = 1_000_000_000;
const NET: NetUid = NetUid(1);

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

fn units(value: u128) -> DynValue {
    DynValue::new(CoreValue::Uint(value))
}

async fn client(state: FakeState) -> Result<(Client, FakeChain)> {
    let fake = FakeChain::from_state(state);
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    Ok((
        Client::from_rpc(RpcClient::new(fake.clone()), config).await?,
        fake,
    ))
}

fn state() -> Result<FakeState> {
    let runtime = FakeState::try_default()?.runtime;
    Ok(intent_state(runtime, "mint_gamma")?)
}

fn any(op: &str, args: Value) -> Result<AnyIntent, IntentError> {
    AnyIntent::from_tool(op, args)
}

async fn build<I: Intent>(intent: &I, snapshot: &Snapshot) -> Result<Vec<u8>, IntentError> {
    let built = intent
        .build(&BuildCx {
            view: snapshot,
            origin: OriginView::Keys(&Keys),
            runtime: &snapshot.block().runtime,
            clock: &SystemClock,
            spec: I::SPEC,
        })
        .await?;
    Ok(built.call.as_bytes().to_vec())
}

fn mint(args: Value) -> MintGamma {
    match any("mint_gamma", args).unwrap() {
        AnyIntent::MintGamma(intent) => intent,
        other => unreachable!("{other:?}"),
    }
}

// pyt: unit/test_gamma.py::test_calls_carry_chain_units
#[tokio::test]
async fn calls_carry_chain_units() -> Result<()> {
    let (client, _) = client(state()?).await?;
    let snapshot = client.at(BlockRef::Best).await?;
    let runtime = &snapshot.block().runtime;
    let compose = |call: rtao_runtime::Call| call.compose(runtime).unwrap().as_bytes().to_vec();
    let hot = HOT.to_string();
    let other = OTHER_HOT.to_string();
    // The fake's defaults: 1 alpha staked, and every mint quotes 400 gamma.
    let intent: MintGamma = mint(json!({"netuid": 1, "hotkey_ss58": hot, "amount_tao": 1.0}));
    assert_eq!(
        build(&intent, &snapshot).await?,
        compose(calls::mint_gamma(
            HOT,
            NET,
            TaoBalance(RAO),
            units(u128::from(380 * RAO)),
            None,
            None
        ))
    );
    let AnyIntent::MintGammaWithStake(intent) = any(
        "mint_gamma_with_stake",
        json!({"netuid": 1, "hotkey_ss58": hot, "origin_hotkey_ss58": other, "amount_alpha": 1.0}),
    )?
    else {
        unreachable!()
    };
    assert_eq!(
        build(&intent, &snapshot).await?,
        compose(calls::mint_gamma_with_stake(
            OTHER_HOT,
            HOT,
            NET,
            AlphaBalance(RAO),
            units(u128::from(380 * RAO)),
            None,
            None
        ))
    );
    for (amount, sent) in [
        (json!("1.5"), 1_500_000_000u128),
        (json!("all"), u128::from(u64::MAX)),
    ] {
        let AnyIntent::MoveGamma(intent) = any(
            "move_gamma",
            json!({"netuid": 1, "origin_hotkey_ss58": other, "destination_hotkey_ss58": hot, "amount_gamma": amount}),
        )?
        else {
            unreachable!()
        };
        assert_eq!(
            build(&intent, &snapshot).await?,
            compose(calls::move_gamma(OTHER_HOT, HOT, NET, units(sent)))
        );
    }
    for (args, cap) in [
        (
            json!({"daily_spend_cap_gamma": "2.5"}),
            Some(2_500_000_000u128),
        ),
        (json!({"daily_spend_cap_gamma": 0}), Some(0)),
        (json!({"remove_cap": true}), None),
    ] {
        let mut args = args;
        args["netuid"] = json!(1);
        args["hotkey_ss58"] = json!(hot);
        let AnyIntent::SetGammaDepositDailySpendCap(intent) =
            any("set_gamma_deposit_daily_spend_cap", args)?
        else {
            unreachable!()
        };
        assert_eq!(
            build(&intent, &snapshot).await?,
            compose(calls::set_gamma_deposit_daily_spend_cap(
                HOT,
                NET,
                cap.map(units)
            ))
        );
    }
    for (args, spender) in [
        (json!({"spender_ss58": COLD.to_string()}), Some(COLD)),
        (json!({"default_spender": true}), None),
    ] {
        let mut args = args;
        args["netuid"] = json!(1);
        args["hotkey_ss58"] = json!(hot);
        let AnyIntent::SetGammaDepositSpender(intent) = any("set_gamma_deposit_spender", args)?
        else {
            unreachable!()
        };
        assert_eq!(
            build(&intent, &snapshot).await?,
            compose(calls::set_gamma_deposit_spender(HOT, NET, spender))
        );
    }
    let AnyIntent::SpendGamma(intent) = any(
        "spend_gamma",
        json!({"netuid": 1, "spends": [{"hotkey_ss58": hot, "amount_gamma": "0.5", "reference": "ab".repeat(32)}]}),
    )?
    else {
        unreachable!()
    };
    assert_eq!(
        intent.spends,
        [
            json!({"hotkey_ss58": hot, "amount_gamma": "0.5", "reference": format!("0x{}", "ab".repeat(32))})
        ]
    );
    assert_eq!(
        build(&intent, &snapshot).await?,
        compose(calls::spend_gamma(
            NET,
            vec![(HOT, units(u128::from(RAO / 2)), [0xab; 32])]
        ))
    );
    let close = CloseGammaDeposit {
        netuid: NET,
        hotkey_ss58: HOT.into(),
    };
    assert_eq!(
        build(&close, &snapshot).await?,
        compose(calls::close_gamma_deposit(HOT, NET))
    );
    client.close().await;
    Ok(())
}

// pyt: unit/test_gamma.py::TestMinOut::test_quote_less_tolerance
// pyt: unit/test_gamma.py::TestMinOut::test_a_refused_quote_raises_the_chain_error
#[tokio::test]
async fn the_minimum_is_the_quote_less_tolerance_and_a_refusal_is_the_chain_error() -> Result<()> {
    for (tolerance, gamma, minimum) in [
        (
            0.1,
            u128::from(400 * RAO),
            (400.0 * RAO as f64 * 0.9) as u128,
        ),
        (0.0, (1u128 << 53) + 3, (1u128 << 53) + 3),
    ] {
        let mut state = state()?;
        state.seed_runtime(
            "GammaRuntimeApi",
            "sim_mint_gamma",
            json!({"Ok": [RAO, RAO, gamma as u64]}),
        )?;
        let (client, _) = client(state).await?;
        let snapshot = client.at(BlockRef::Best).await?;
        let intent = mint(
            json!({"netuid": 1, "hotkey_ss58": HOT.to_string(), "amount_tao": 1.0, "rate_tolerance": tolerance}),
        );
        let expected = calls::mint_gamma(HOT, NET, TaoBalance(RAO), units(minimum), None, None)
            .compose(&snapshot.block().runtime)?;
        assert_eq!(build(&intent, &snapshot).await?, expected.as_bytes());
        client.close().await;
    }
    let error = rtao_runtime::errors::MODULE_ERRORS
        .iter()
        .find(|error| error.pallet == "SubtensorModule" && error.name == "GammaDisabled")
        .unwrap();
    let mut state = state()?;
    state.seed_runtime(
        "GammaRuntimeApi",
        "sim_mint_gamma",
        json!({"Err": {"Module": {"index": error.pallet_index, "error": [error.error_index, 0, 0, 0]}}}),
    )?;
    let (client, _) = client(state).await?;
    let snapshot = client.at(BlockRef::Best).await?;
    let refused = build(
        &mint(json!({"netuid": 1, "hotkey_ss58": HOT.to_string(), "amount_tao": 1.0})),
        &snapshot,
    )
    .await;
    assert!(
        matches!(&refused, Err(IntentError::Chain(chain)) if chain.name.as_deref() == Some("GammaDisabled")),
        "{refused:?}"
    );
    client.close().await;
    Ok(())
}

// pyt: unit/test_gamma.py::TestMintWithStake::test_quote_clamped_to_stake_held
// pyt: unit/test_gamma.py::TestMintWithStake::test_no_stake_refuses
#[tokio::test]
async fn a_stake_mint_quotes_at_most_the_stake_held_and_refuses_without_stake() -> Result<()> {
    for (amount, sent) in [(json!(1.0), RAO), (json!("all"), u64::MAX)] {
        let mut state = state()?;
        state.seed_runtime(
            "StakeInfoRuntimeApi",
            "get_stake_info_for_hotkey_coldkey_netuid",
            json!({"hotkey": OTHER_HOT.to_string(), "coldkey": COLD.to_string(), "netuid": 1, "stake": RAO / 4, "locked": 0, "emission": 0, "tao_emission": 0, "drain": 0, "is_registered": true}),
        )?;
        state.seed_runtime(
            "GammaRuntimeApi",
            "sim_mint_gamma_with_stake",
            json!({"Ok": [RAO / 4, RAO / 4, 100 * RAO]}),
        )?;
        let (client, chain) = client(state).await?;
        let snapshot = client.at(BlockRef::Best).await?;
        let AnyIntent::MintGammaWithStake(intent) = any(
            "mint_gamma_with_stake",
            json!({"netuid": 1, "hotkey_ss58": HOT.to_string(), "origin_hotkey_ss58": OTHER_HOT.to_string(), "amount_alpha": amount}),
        )?
        else {
            unreachable!()
        };
        let expected = calls::mint_gamma_with_stake(
            OTHER_HOT,
            HOT,
            NET,
            AlphaBalance(sent),
            units((100.0 * RAO as f64 * 0.95) as u128),
            None,
            None,
        )
        .compose(&snapshot.block().runtime)?;
        assert_eq!(build(&intent, &snapshot).await?, expected.as_bytes());
        // The quote asked for the stake held, not the request.
        let mut quoted = 1u16.to_le_bytes().to_vec();
        quoted.extend((RAO / 4).to_le_bytes());
        let quoted = json!(format!("0x{}", hex::encode(quoted)));
        assert!(
            chain
                .requests()
                .iter()
                .any(|request| request.method == "state_call"
                    && request.params[0] == json!("GammaRuntimeApi_sim_mint_gamma_with_stake")
                    && request.params[1] == quoted)
        );
        client.close().await;
    }
    let mut state = state()?;
    state.seed_runtime(
        "StakeInfoRuntimeApi",
        "get_stake_info_for_hotkey_coldkey_netuid",
        Value::Null,
    )?;
    let (client, _) = client(state).await?;
    let snapshot = client.at(BlockRef::Best).await?;
    let intent = MintGammaWithStake {
        netuid: NET,
        hotkey_ss58: HOT.into(),
        amount_alpha: rtao_primitives::Amount::parse("1")?,
        origin_hotkey_ss58: OTHER_HOT.into(),
        rate_tolerance: rtao_primitives::Ratio::new(0.05)?,
        spender_ss58: None,
        service_hotkey_ss58: None,
        daily_spend_cap_gamma: None,
    };
    let refused = build(&intent, &snapshot).await.unwrap_err().to_string();
    assert!(refused.contains("no stake"), "{refused}");
    client.close().await;
    Ok(())
}

// pyt: unit/test_gamma.py::TestSpendContract::test_spend_is_unbounded
#[test]
fn spends_are_bounded_only_for_tao_mints() -> Result<()> {
    let hot = HOT.to_string();
    let other = OTHER_HOT.to_string();
    assert!(matches!(
        mint(json!({"netuid": 1, "hotkey_ss58": hot, "amount_tao": 1.0})).spend(),
        Spend::Bounded(amount) if amount.rao() == RAO
    ));
    for (op, args) in [
        (
            "mint_gamma_with_stake",
            json!({"netuid": 1, "hotkey_ss58": hot, "origin_hotkey_ss58": other, "amount_alpha": 1.0}),
        ),
        (
            "move_gamma",
            json!({"netuid": 1, "origin_hotkey_ss58": other, "destination_hotkey_ss58": hot, "amount_gamma": 1}),
        ),
        (
            "set_gamma_deposit_spender",
            json!({"netuid": 1, "hotkey_ss58": hot, "spender_ss58": COLD.to_string()}),
        ),
    ] {
        assert_eq!(
            any(op, args)?.as_dyn().spend_dyn(),
            Spend::Unbounded,
            "{op}"
        );
    }
    Ok(())
}

#[test]
fn inputs_are_checked_as_python_constructs_them() {
    let hot = HOT.to_string();
    let other = OTHER_HOT.to_string();
    for (op, args, needle) in [
        (
            "move_gamma",
            json!({"netuid": 1, "origin_hotkey_ss58": hot, "destination_hotkey_ss58": hot, "amount_gamma": 1}),
            "are the same",
        ),
        (
            "move_gamma",
            json!({"netuid": 1, "origin_hotkey_ss58": other, "destination_hotkey_ss58": hot, "amount_gamma": 0}),
            "above 0",
        ),
        (
            "mint_gamma",
            json!({"netuid": 1, "hotkey_ss58": hot, "amount_tao": 1, "spender_ss58": other, "service_hotkey_ss58": other}),
            "not both",
        ),
        (
            "mint_gamma",
            json!({"netuid": 1, "hotkey_ss58": hot, "amount_tao": 1, "rate_tolerance": 1.0}),
            "[0, 1)",
        ),
        (
            "set_gamma_deposit_spender",
            json!({"netuid": 1, "hotkey_ss58": hot}),
            "exactly one",
        ),
        (
            "set_gamma_deposit_spender",
            json!({"netuid": 1, "hotkey_ss58": hot, "spender_ss58": other, "default_spender": true}),
            "exactly one",
        ),
        (
            "set_gamma_deposit_daily_spend_cap",
            json!({"netuid": 1, "hotkey_ss58": hot}),
            "exactly one",
        ),
        (
            "set_gamma_deposit_daily_spend_cap",
            json!({"netuid": 1, "hotkey_ss58": hot, "daily_spend_cap_gamma": 1, "remove_cap": true}),
            "exactly one",
        ),
        (
            "spend_gamma",
            json!({"netuid": 1, "spends": []}),
            "1 to 512",
        ),
        (
            "spend_gamma",
            json!({"netuid": 1, "spends": [{"hotkey_ss58": hot}]}),
            "entry 1 must be",
        ),
        (
            "spend_gamma",
            json!({"netuid": 1, "spends": [{"hotkey_ss58": hot, "amount_gamma": 1, "reference": "ab"}]}),
            "32 bytes",
        ),
        (
            "spend_gamma",
            json!({"netuid": 1, "spends": [{"hotkey_ss58": hot, "amount_gamma": "0.0000000001"}]}),
            "at most 9 decimals",
        ),
    ] {
        let error = any(op, args).unwrap_err().to_string();
        assert!(error.contains(needle), "{op}: {error}");
    }
}

#[test]
fn summaries_name_the_terms() {
    let hot = HOT.to_string();
    let intent = mint(json!({"netuid": 1, "hotkey_ss58": hot, "amount_tao": 1.0}));
    assert!(
        intent
            .summary()
            .ends_with("(fails if the quote falls >5.00%)"),
        "{}",
        intent.summary()
    );
    let AnyIntent::SpendGamma(spend) = any(
        "spend_gamma",
        json!({"netuid": 1, "spends": [{"hotkey_ss58": hot, "amount_gamma": "0.5"}, {"hotkey_ss58": hot, "amount_gamma": 1}]}),
    )
    .unwrap() else {
        unreachable!()
    };
    assert_eq!(spend.summary(), "spend 1.5 gamma in 2 entries on netuid 1");
    let cap = SetGammaDepositDailySpendCap {
        netuid: NET,
        hotkey_ss58: HOT.into(),
        daily_spend_cap_gamma: None,
        remove_cap: true,
    };
    assert_eq!(
        cap.summary(),
        format!("set no cap on the gamma deposit at {hot} on netuid 1")
    );
    let spender = SetGammaDepositSpender {
        netuid: NET,
        hotkey_ss58: HOT.into(),
        spender_ss58: None,
        service_hotkey_ss58: None,
        default_spender: true,
    };
    assert!(
        spender
            .summary()
            .starts_with("let the subnet owner's service spender")
    );
    let moved = MoveGamma {
        netuid: NET,
        origin_hotkey_ss58: OTHER_HOT.into(),
        destination_hotkey_ss58: HOT.into(),
        amount_gamma: rtao_primitives::Amount::All,
    };
    assert!(moved.summary().starts_with("move all gamma on netuid 1"));
}

struct PublicKeys;

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

impl rtao_signer::DynKeySource for PublicKeys {
    #[allow(clippy::panic)] // Planning must never acquire a private signer.
    fn signer_for<'a>(
        &'a self,
        role: rtao_signer::SignerRole,
    ) -> rtao_signer::BoxFuture<'a, Result<std::sync::Arc<dyn rtao_signer::Signer>, SignerError>>
    {
        panic!("planning attempted to acquire a private {role} signer");
    }
}

fn deposit(coldkey: AccountId, spender: AccountId, balance: u64, cap: Option<u64>) -> Value {
    json!({
        "coldkey": coldkey.to_string(),
        "spender": spender.to_string(),
        "balance": balance,
        "daily_spend_cap": cap.map(|cap| json!({"cap": cap, "bucket": [0, 0]})),
    })
}

async fn mint_plan(
    seed: impl FnOnce(&mut FakeState) -> Result<()>,
    args: Value,
) -> Result<rtao_tx::IntentPreflight> {
    let mut state = FakeState::try_default()?;
    state.load_python_defaults()?;
    state.seed_default("System", "Account", json!({"data": {"free": 100 * RAO}}))?;
    seed(&mut state)?;
    let (client, _) = client(state).await?;
    let mut base = json!({"netuid": 1, "hotkey_ss58": HOT.to_string(), "amount_tao": 1.0});
    for (key, value) in args.as_object().into_iter().flatten() {
        base[key] = value.clone();
    }
    let intent = any("mint_gamma", base)?;
    let plan = rtao_tx::Executor::new(client.clone())
        .preflight_dyn(
            intent.as_dyn(),
            &PublicKeys,
            &rtao_tx::PlanOptions::default(),
        )
        .await?;
    client.close().await;
    Ok(plan)
}

fn seed_deposit(state: &mut FakeState, value: Value) -> Result<()> {
    state.seed_python(
        "SubtensorModule",
        "GammaDeposits",
        &[json!(1), json!(HOT.to_string())],
        value,
    )?;
    Ok(())
}

fn fact(plan: &rtao_tx::IntentPreflight, name: &str) -> Option<String> {
    plan.facts
        .iter()
        .find(|(fact, _)| fact == name)
        .map(|(_, value)| value.clone())
}

// pyt: unit/test_gamma.py::TestMintPreview::test_quote_facts
// pyt: unit/test_gamma.py::TestMintPreview::test_another_coldkeys_deposit_blocks
// pyt: unit/test_gamma.py::TestMintPreview::test_a_new_deposit_requires_the_hotkey_owner
#[tokio::test]
async fn a_mint_preview_shows_the_quote_and_the_deposit_it_credits() -> Result<()> {
    let plan = mint_plan(
        |state| {
            state.seed_python(
                "SubtensorModule",
                "Owner",
                &[json!(HOT.to_string())],
                json!(COLD.to_string()),
            )?;
            state.seed_runtime(
                "GammaRuntimeApi",
                "sim_mint_gamma",
                json!({"Ok": [RAO, RAO / 2, 200 * RAO]}),
            )?;
            Ok(())
        },
        json!({}),
    )
    .await?;
    let intent: MintGamma =
        mint(json!({"netuid": 1, "hotkey_ss58": HOT.to_string(), "amount_tao": 1.0}));
    assert_eq!(plan.effects, [intent.summary()]);
    assert!(plan.blocks.is_empty(), "{:?}", plan.blocks);
    assert!(fact(&plan, "gamma").unwrap().contains("200"));
    assert!(fact(&plan, "minimum").unwrap().contains("190"));
    assert_eq!(
        Value::Object(plan.preview.clone()),
        json!({
            "gamma": "200",
            "minimum_gamma": "190",
            "alpha_burned": "1",
            "tao_value": "0.5",
            "deposit_balance_gamma": null,
            "spender_ss58": null,
            "daily_spend_cap_gamma": null,
        })
    );
    // Another coldkey's deposit blocks the mint.
    let plan = mint_plan(
        |state| seed_deposit(state, deposit(HOT, HOT, 0, None)),
        json!({}),
    )
    .await?;
    assert!(
        plan.blocks
            .iter()
            .any(|block| block.contains("belongs to another coldkey"))
    );
    // A new deposit needs the signer to own the hotkey.
    for (owner, blocked) in [(Some(COLD), false), (Some(OTHER_HOT), true), (None, true)] {
        let plan = mint_plan(
            |state| {
                if let Some(owner) = owner {
                    state.seed_python(
                        "SubtensorModule",
                        "Owner",
                        &[json!(HOT.to_string())],
                        json!(owner.to_string()),
                    )?;
                }
                Ok(())
            },
            json!({}),
        )
        .await?;
        assert_eq!(
            plan.blocks.iter().any(|block| block.contains("associate")),
            blocked,
            "{owner:?}: {:?}",
            plan.blocks
        );
    }
    Ok(())
}

// pyt: unit/test_gamma.py::TestMintTerms::test_a_deposit_naming_another_spender_blocks
// pyt: unit/test_gamma.py::TestMintTerms::test_a_deposit_with_another_cap_blocks
// pyt: unit/test_gamma.py::TestMintTerms::test_a_new_deposit_shows_the_named_cap
// pyt: unit/test_gamma.py::TestMintTerms::test_a_service_names_its_spender
#[tokio::test]
async fn a_mint_must_match_the_deposits_spender_and_cap() -> Result<()> {
    for (spender, blocked) in [(None, false), (Some(COLD), false), (Some(OTHER_HOT), true)] {
        let plan = mint_plan(
            |state| seed_deposit(state, deposit(COLD, COLD, 0, None)),
            json!({"spender_ss58": spender.map(|spender| spender.to_string())}),
        )
        .await?;
        assert_eq!(
            plan.blocks
                .iter()
                .any(|block| block.contains("is spent by")),
            blocked,
            "{spender:?}"
        );
    }
    for (held, named, blocked) in [
        (None, None, false),
        (Some(RAO), None, false),
        (Some(RAO), Some(1), false),
        (Some(RAO), Some(2), true),
        (None, Some(0), true),
    ] {
        let plan = mint_plan(
            |state| seed_deposit(state, deposit(COLD, COLD, 0, held)),
            json!({"daily_spend_cap_gamma": named}),
        )
        .await?;
        assert_eq!(
            plan.blocks
                .iter()
                .any(|block| block.contains("daily spend cap")),
            blocked,
            "{held:?} {named:?}"
        );
        assert_eq!(
            fact(&plan, "daily spend cap").unwrap(),
            if held.is_none() { "none" } else { "1 gamma" }
        );
    }
    let plan = mint_plan(|_| Ok(()), json!({"daily_spend_cap_gamma": "2.5"})).await?;
    assert_eq!(fact(&plan, "daily spend cap").unwrap(), "2.5 gamma");
    let plan = mint_plan(
        |state| {
            state.seed_python(
                "SubtensorModule",
                "Services",
                &[json!(1), json!(OTHER_HOT.to_string())],
                json!({"url": "https://a.example", "spender": OTHER_HOT.to_string()}),
            )?;
            Ok(())
        },
        json!({"service_hotkey_ss58": OTHER_HOT.to_string()}),
    )
    .await?;
    assert_eq!(fact(&plan, "spender").unwrap(), OTHER_HOT.to_string());
    let missing = mint_plan(|_| Ok(()), json!({"service_hotkey_ss58": COLD.to_string()})).await;
    assert!(
        missing
            .as_ref()
            .is_err_and(|error| error.to_string().contains("publishes no service")),
        "{missing:?}"
    );
    Ok(())
}
