//! Compare 21 actual M2/M4/M5 intents with pinned trusted core constructors.
//!
//! Core diagnostics use integer rao, while rtao preserves Python's rendered
//! balances. Compare fee/spend/global diagnostic categories in order and subnet
//! violation sets. Python retains origin/destination ordering and duplicates;
//! the core canonicalizes its private scope. Both tails are asserted explicitly.
//! Amounts outside native u64 are intentionally excluded (Q25).
#![allow(clippy::unwrap_used, clippy::panic)]

use rtao_client::{BlockRef, Client, ClientConfig, DynView, Network};
use rtao_core::value_ext::ValueExt;
use rtao_intents::{
    AddStake, AddStakeLimit, BurnedRegister, FundEvmKey, MoveStake, RegisterSubnet, RemoveStake,
    RemoveStakeLimit, RootRegister, SetHyperparameter, SetTake, StartCall, SwapStake, Transfer,
    TransferAll, TransferStake, UnstakeAll, UnstakeAllAlpha,
};
use rtao_primitives::{
    AccountId, Alpha, Amount, Balance, DynBalance, NetUid, PerU16, Ratio, Spend, Tao, UnitError,
};
use rtao_signer::{BoxFuture, DynKeySource, KeyView, Signer, SignerError, SignerRole};
use rtao_testkit::core_transaction::{
    IntentCall, Policy as CorePolicy, SignerRole as CoreSignerRole,
};
use rtao_tx::{
    BuildCx, DynIntent, ExecuteOptions, Executor, Intent, IntentError, OriginView, PlanOptions,
    Policy, SystemClock, TxError,
};
use serde_json::{Value, json};
use std::{collections::BTreeSet, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::oneshot,
    task::JoinHandle,
};

const DESTINATION: AccountId = AccountId([42; 32]);
const OWNER: AccountId = AccountId([43; 32]);
const DELEGATE: AccountId = AccountId([44; 32]);
const M4_NETUIDS: [u16; 4] = [0, 1, 63, u16::MAX];
const AMOUNTS: [u64; 10] = [
    0,
    1,
    9,
    10,
    11,
    9_007_199_254_740_991,
    9_007_199_254_740_992,
    9_007_199_254_740_993,
    u64::MAX - 1,
    u64::MAX,
];
const CAPS: [Option<u64>; 8] = [
    None,
    Some(0),
    Some(1),
    Some(10),
    Some(9_007_199_254_740_992),
    Some(9_007_199_254_740_993),
    Some(u64::MAX - 1),
    Some(u64::MAX),
];
const FEES: [Option<u64>; 8] = [
    None,
    Some(0),
    Some(1),
    Some(9),
    Some(10),
    Some(11),
    Some(9_007_199_254_740_993),
    Some(u64::MAX),
];

struct Pair {
    constructor: &'static str,
    core: IntentCall,
    intent: Box<dyn DynIntent>,
}

// This table constructs both real implementations. No substitute PolicySubject
// describes the rtao side, and no arbitrary core call supplies trusted semantics.
fn m2_pairs() -> Vec<Pair> {
    let mut pairs = Vec::new();
    for amount in AMOUNTS {
        for keep_alive in [true, false] {
            pairs.push(Pair {
                constructor: if keep_alive {
                    "transfer"
                } else {
                    "transfer_allow_death"
                },
                core: if keep_alive {
                    IntentCall::transfer(DESTINATION.to_string(), u128::from(amount))
                } else {
                    IntentCall::transfer_allow_death(DESTINATION.to_string(), u128::from(amount))
                },
                intent: Box::new(Transfer {
                    dest_ss58: DESTINATION.into(),
                    amount_tao: Balance::<Tao>::from_rao(amount).into(),
                    keep_alive,
                }),
            });
        }
    }
    for keep_alive in [true, false] {
        pairs.push(Pair {
            constructor: "transfer_all",
            core: IntentCall::transfer_all(DESTINATION.to_string(), keep_alive),
            intent: Box::new(TransferAll {
                dest_ss58: DESTINATION.into(),
                keep_alive,
            }),
        });
        // Python also exposes the sweep through Transfer(amount_tao="all").
        pairs.push(Pair {
            constructor: "transfer_all",
            core: IntentCall::transfer_all(DESTINATION.to_string(), keep_alive),
            intent: Box::new(Transfer {
                dest_ss58: DESTINATION.into(),
                amount_tao: Amount::All,
                keep_alive,
            }),
        });
    }
    pairs
}

// Core's owner constructor covers this shared subset of the Python allowlist.
// Inputs and expected raw values are independent constants, including human
// TAO conversion and the U64F64 record shape. The 34-row planner tests cover
// settings the core constructor does not support.
fn m4_hyperparameters() -> Vec<(&'static str, Value, rtao_core::codec::Value)> {
    use rtao_core::codec::Value as CoreValue;
    let mut values = vec![
        ("immunity_period", json!(10), CoreValue::Uint(10)),
        ("min_allowed_weights", json!(1), CoreValue::Uint(1)),
        (
            "weights_version",
            json!(u64::MAX),
            CoreValue::Uint(u128::from(u64::MAX)),
        ),
        ("min_burn", json!("0.000000011"), CoreValue::Uint(11)),
        ("bonds_moving_avg", json!(900_000), CoreValue::Uint(900_000)),
        ("serving_rate_limit", json!(7), CoreValue::Uint(7)),
        ("commit_reveal_period", json!(1), CoreValue::Uint(1)),
        ("max_allowed_uids", json!(64), CoreValue::Uint(64)),
        (
            "burn_increase_mult",
            json!("1.5"),
            CoreValue::record(vec![("bits".into(), CoreValue::Uint(3u128 << 63))]),
        ),
        ("burn_half_life", json!(600), CoreValue::Uint(600)),
    ];
    for name in [
        "commit_reveal_weights_enabled",
        "liquid_alpha_enabled",
        "network_pow_registration_allowed",
        "yuma3_enabled",
        "bonds_reset_enabled",
        "transfers_enabled",
        "owner_cut_enabled",
        "owner_cut_auto_lock_enabled",
    ] {
        for enabled in [false, true] {
            values.push((name, json!(enabled), CoreValue::Bool(enabled)));
        }
    }
    values
}

fn m4_pairs() -> Vec<Pair> {
    let mut pairs = vec![];
    // A wallet default and an explicit address must reach the same core call.
    for hotkey_ss58 in [None, Some(DESTINATION.into())] {
        pairs.push(Pair {
            constructor: "register_subnet",
            core: IntentCall::register_subnet(DESTINATION.to_string()),
            intent: Box::new(RegisterSubnet {
                hotkey_ss58: hotkey_ss58.clone(),
            }),
        });
        pairs.push(Pair {
            constructor: "root_register",
            core: IntentCall::root_register(DESTINATION.to_string()),
            intent: Box::new(RootRegister { hotkey_ss58 }),
        });
    }
    for netuid in M4_NETUIDS {
        pairs.push(Pair {
            constructor: "start_call",
            core: IntentCall::start_call(netuid),
            intent: Box::new(StartCall {
                netuid: NetUid(netuid),
            }),
        });
        for hotkey_ss58 in [None, Some(DESTINATION.into())] {
            pairs.push(Pair {
                constructor: "burned_register",
                core: IntentCall::burned_register(netuid, DESTINATION.to_string()),
                intent: Box::new(BurnedRegister {
                    netuid: NetUid(netuid),
                    hotkey_ss58,
                }),
            });
        }
        for (name, value, raw) in m4_hyperparameters() {
            pairs.push(Pair {
                constructor: "set_hyperparameter",
                core: IntentCall::set_hyperparameter(netuid, name, raw).unwrap(),
                intent: Box::new(SetHyperparameter {
                    netuid: NetUid(netuid),
                    name: name.into(),
                    value,
                }),
            });
        }
    }
    pairs
}

// These test keys contain public accounts only. Any attempt to unlock during
// call construction, fee probing or proxy planning fails the test.
struct PublicKeys(AccountId);
impl KeyView for PublicKeys {
    fn hotkey_address(&self) -> Result<AccountId, SignerError> {
        Ok(DESTINATION)
    }
    fn hotkey_public_key(&self) -> Result<[u8; 32], SignerError> {
        Ok(DESTINATION.0)
    }
    fn coldkey_address(&self) -> Result<AccountId, SignerError> {
        Ok(self.0)
    }
}
impl DynKeySource for PublicKeys {
    fn signer_for<'a>(
        &'a self,
        _role: SignerRole,
    ) -> BoxFuture<'a, Result<Arc<dyn Signer>, SignerError>> {
        panic!("policy preview must never unlock a signing key")
    }
}

async fn m4_fixture() -> (Client, rtao_testkit::FakeChain) {
    use rtao_runtime::storage::subtensor_module as st;
    let mut state = rtao_testkit::FakeState::default();
    state.seed(&st::ADMIN_FREEZE_WINDOW, (), 0).unwrap();
    state.seed(&st::OWNER_HYPERPARAM_RATE_LIMIT, (), 0).unwrap();
    for netuid in M4_NETUIDS.map(NetUid) {
        state
            .seed(&st::SUBNET_OWNER, (netuid,), OWNER.into())
            .unwrap();
        state.seed(&st::TEMPO, (netuid,), 0).unwrap();
    }
    let fake = rtao_testkit::FakeChain::from_state(state);
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None).unwrap()).unwrap();
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    let client = Client::from_rpc(rtao_testkit::rpc::RpcClient::new(fake.clone()), config)
        .await
        .unwrap();
    (client, fake)
}

fn core_policy(policy: &Policy) -> CorePolicy {
    CorePolicy {
        max_fee_rao: policy.max_fee.map(|value| u128::from(value.rao())),
        max_spend_rao: policy.max_spend.map(|value| u128::from(value.rao())),
        allowed_netuids: policy
            .allowed_netuids
            .as_ref()
            .map(|allowed| allowed.iter().map(|netuid| netuid.0).collect()),
        allow_raw_calls: policy.allow_raw_calls,
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Violation {
    RawCall,
    FeeUnavailable,
    FeeExceeded,
    SpendUnbounded,
    SpendExceeded,
    AllSubnets,
    Netuid(u16),
}

fn categories(messages: &[String]) -> Vec<Violation> {
    messages
        .iter()
        .map(|message| {
            if message.starts_with("raw call") {
                Violation::RawCall
            } else if message.contains("max_fee_") {
                if message.contains("could not be estimated")
                    || message.contains("estimation failed")
                {
                    Violation::FeeUnavailable
                } else if message.contains("exceeds") {
                    Violation::FeeExceeded
                } else {
                    panic!("unrecognized fee diagnostic: {message}")
                }
            } else if message.contains("max_spend_") {
                if message.contains("unbounded") || message.contains("cannot be bounded") {
                    Violation::SpendUnbounded
                } else if message.contains("exceeds") {
                    Violation::SpendExceeded
                } else {
                    panic!("unrecognized spend diagnostic: {message}")
                }
            } else if message.contains("all subnets") || message.contains("every subnet") {
                Violation::AllSubnets
            } else if message.starts_with("netuid ") {
                Violation::Netuid(message.split_whitespace().nth(1).unwrap().parse().unwrap())
            } else {
                panic!("unrecognized policy diagnostic: {message}")
            }
        })
        .collect()
}

#[test]
fn trusted_m2_constructors_have_identical_policy_violation_sets() {
    let pairs = m2_pairs();
    assert_eq!(
        pairs
            .iter()
            .map(|pair| pair.constructor)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["transfer", "transfer_allow_death", "transfer_all"])
    );
    let allowlists = [
        None,
        Some(BTreeSet::new()),
        Some(BTreeSet::from([0, 2, 63])),
    ];
    let mut checks = 0;
    for pair in pairs {
        pair.intent.validate_dyn().unwrap();
        assert_eq!(pair.core.pallet(), "Balances");
        assert_eq!(pair.core.signer_role(), CoreSignerRole::Coldkey);
        assert_eq!(pair.intent.spec().signer, "coldkey");
        for max_fee in CAPS {
            for max_spend in CAPS {
                for allowed in &allowlists {
                    for allow_raw_calls in [false, true] {
                        let core_policy = CorePolicy {
                            max_fee_rao: max_fee.map(u128::from),
                            max_spend_rao: max_spend.map(u128::from),
                            allowed_netuids: allowed.clone(),
                            allow_raw_calls,
                        };
                        let policy = Policy {
                            max_fee: max_fee.map(Balance::<Tao>::from_rao),
                            max_spend: max_spend.map(Balance::<Tao>::from_rao),
                            allowed_netuids: allowed
                                .as_ref()
                                .map(|uids| uids.iter().copied().map(NetUid).collect()),
                            allow_raw_calls,
                        };
                        for fee in FEES {
                            let core = core_policy.check(&pair.core, fee.map(u128::from));
                            let actual = policy.check(
                                pair.intent.semantic_intent_dyn(),
                                fee.map(Balance::<Tao>::from_rao),
                            );
                            assert_eq!(
                                categories(&actual),
                                categories(&core),
                                "{} {:?}, fee={fee:?}, core_policy={core_policy:?}",
                                pair.constructor,
                                pair.intent.to_json_dyn()
                            );
                            assert_eq!(actual.is_empty(), core.is_empty());
                            checks += 1;
                        }
                    }
                }
            }
        }
    }
    assert_eq!(checks, 73_728);
}

#[test]
fn trusted_m4_constructors_have_identical_policy_violation_sets() {
    let pairs = m4_pairs();
    assert_eq!(pairs.len(), 120);
    assert_eq!(
        pairs
            .iter()
            .map(|pair| pair.constructor)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([
            "register_subnet",
            "start_call",
            "burned_register",
            "root_register",
            "set_hyperparameter",
        ])
    );
    let allowlists = [
        None,
        Some(BTreeSet::new()),
        Some(BTreeSet::from([0])),
        Some(BTreeSet::from([1, 63])),
        Some(BTreeSet::from(M4_NETUIDS)),
    ];
    let mut checks = 0;
    for pair in pairs {
        pair.intent.validate_dyn().unwrap();
        assert_eq!(pair.core.signer_role(), CoreSignerRole::Coldkey);
        assert_eq!(pair.intent.spec().signer, "coldkey");
        assert_eq!(
            pair.intent.spec().origin,
            if matches!(pair.constructor, "start_call" | "set_hyperparameter") {
                "subnet_owner"
            } else {
                "signed"
            }
        );
        for max_fee in CAPS {
            for max_spend in CAPS {
                for allowed in &allowlists {
                    for allow_raw_calls in [false, true] {
                        let policy = Policy {
                            max_fee: max_fee.map(Balance::<Tao>::from_rao),
                            max_spend: max_spend.map(Balance::<Tao>::from_rao),
                            allowed_netuids: allowed
                                .as_ref()
                                .map(|uids| uids.iter().copied().map(NetUid).collect()),
                            allow_raw_calls,
                        };
                        let core_policy = core_policy(&policy);
                        for fee in FEES {
                            let core = core_policy.check(&pair.core, fee.map(u128::from));
                            let actual = policy.check(
                                pair.intent.semantic_intent_dyn(),
                                fee.map(Balance::<Tao>::from_rao),
                            );
                            assert_eq!(
                                categories(&actual),
                                categories(&core),
                                "{} {:?}, fee={fee:?}, core_policy={core_policy:?}",
                                pair.constructor,
                                pair.intent.to_json_dyn()
                            );
                            assert_eq!(actual.is_empty(), core.is_empty());
                            checks += 1;
                        }
                    }
                }
            }
        }
    }
    assert_eq!(checks, 614_400);
}

#[test]
fn m4_same_call_raw_controls_require_the_explicit_opt_in() {
    for pair in m4_pairs() {
        let raw = pair.core.clone().raw();
        assert_eq!(raw.pallet(), pair.core.pallet());
        assert_eq!(raw.function(), pair.core.function());
        assert_eq!(raw.params(), pair.core.params());
        for allow_raw_calls in [false, true] {
            // Python's raw executor path checks only its explicit opt-in.
            // Core additionally enforces fee/spend/scope caps on raw calls;
            // use neutral caps here so both actual raw gates are comparable.
            let policy = Policy {
                allow_raw_calls,
                ..Default::default()
            };
            assert_eq!(
                categories(&policy.check_raw_call()),
                categories(&core_policy(&policy).check(&raw, None)),
                "{} raw opt-in={allow_raw_calls}",
                pair.constructor
            );
        }
    }
}

#[tokio::test]
async fn trusted_m4_policy_pairs_build_identical_chain_calls() {
    let (client, fake) = m4_fixture().await;
    let snapshot = client.at(BlockRef::Finalized).await.unwrap();
    let runtime = &snapshot.block().runtime;
    let keys = PublicKeys(OWNER);
    for pair in m4_pairs() {
        // A public wallet view exercises defaults; a substituted public account
        // exercises the origin used by proxy dispatch. Explicit hotkeys remain
        // available when that account carries no attached wallet hotkey.
        for origin in [OriginView::Keys(&keys), OriginView::Account(OWNER)] {
            if matches!(origin, OriginView::Account(_))
                && pair.intent.to_json_dyn().get("hotkey_ss58") == Some(&Value::Null)
            {
                continue;
            }
            let cx = BuildCx {
                view: &snapshot as &dyn DynView,
                origin,
                runtime,
                clock: &SystemClock,
                spec: pair.intent.spec(),
            };
            let built = pair.intent.build_dyn(&cx).await.unwrap();
            let expected = runtime
                .core()
                .compose_call(pair.core.pallet(), pair.core.function(), pair.core.params())
                .unwrap();
            assert_eq!(
                built.call.as_bytes(),
                expected,
                "{} {:?}",
                pair.constructor,
                pair.intent.to_json_dyn()
            );
        }
    }
    assert!(fake.state().lock().unwrap().submissions.is_empty());
    client.close().await;
}

#[tokio::test]
async fn m4_raw_executor_rejects_the_same_calls_before_unlock_or_submission() {
    let (client, fake) = m4_fixture().await;
    let executor = Executor::new(client);
    let snapshot = executor.client().at(BlockRef::Finalized).await.unwrap();
    let runtime = &snapshot.block().runtime;
    let keys = PublicKeys(OWNER);
    let mut seen = BTreeSet::new();
    for pair in m4_pairs() {
        if !seen.insert(pair.constructor) {
            continue;
        }
        let built = pair
            .intent
            .build_dyn(&BuildCx {
                view: &snapshot as &dyn DynView,
                origin: OriginView::Keys(&keys),
                runtime,
                clock: &SystemClock,
                spec: pair.intent.spec(),
            })
            .await
            .unwrap();
        let error = executor
            .submit_call_dyn(
                built.call,
                &keys,
                SignerRole::Coldkey,
                &ExecuteOptions {
                    policy: Some(Policy::default()),
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();
        let TxError::Policy(error) = error else {
            panic!(
                "expected the raw policy gate to reject {}",
                pair.constructor
            );
        };
        assert_eq!(
            categories(&error.violations),
            categories(&CorePolicy::default().check(&pair.core.raw(), None))
        );
    }
    assert_eq!(seen.len(), 5);
    assert!(fake.state().lock().unwrap().submissions.is_empty());
    executor.client().close().await;
}

#[tokio::test]
async fn m4_actual_direct_and_proxy_plans_preserve_call_and_policy_semantics() {
    use rtao_core::value_ext::ValueExt;
    let (client, fake) = m4_fixture().await;
    let executor = Executor::new(client);
    let snapshot = executor.client().at(BlockRef::Finalized).await.unwrap();
    let runtime = &snapshot.block().runtime;
    let mut seen = BTreeSet::new();
    // Defaults are exercised above. Use explicit hotkeys here so proxy plans
    // need only the effective account, matching Python's proxy-key behavior.
    for pair in m4_pairs().into_iter().filter(|pair| {
        let args = pair.intent.to_json_dyn();
        args.get("hotkey_ss58") != Some(&Value::Null)
            && args.get("netuid").is_none_or(|netuid| netuid == 63)
    }) {
        if !seen.insert(pair.constructor) {
            continue;
        }
        let expected_inner = runtime
            .core()
            .compose_call(pair.core.pallet(), pair.core.function(), pair.core.params())
            .unwrap();
        for proxy_for in [None, Some(OWNER)] {
            // The delegate is deliberately not the stored owner. This catches
            // using the outer payer instead of the real dispatch identity.
            let keys = PublicKeys(if proxy_for.is_some() { DELEGATE } else { OWNER });
            for policy in [
                Policy::default(),
                Policy {
                    max_fee: Some(Balance::<Tao>::ZERO),
                    max_spend: Some(Balance::<Tao>::ZERO),
                    allowed_netuids: Some(BTreeSet::new()),
                    allow_raw_calls: false,
                },
                Policy {
                    max_fee: Some(Balance::<Tao>::from_rao(u64::MAX)),
                    max_spend: Some(Balance::<Tao>::from_rao(u64::MAX)),
                    allowed_netuids: Some(M4_NETUIDS.map(NetUid).into_iter().collect()),
                    allow_raw_calls: true,
                },
            ] {
                let plan = executor
                    .plan_dyn(
                        pair.intent.as_ref(),
                        &keys,
                        &PlanOptions {
                            policy: Some(policy.clone()),
                            proxy_for,
                            ..Default::default()
                        },
                    )
                    .await
                    .unwrap();
                assert_eq!(plan.signer_address, keys.0);
                assert_eq!(
                    categories(&plan.violations),
                    categories(
                        &core_policy(&policy)
                            .check(&pair.core, plan.fee.map(|fee| u128::from(fee.rao())))
                    ),
                    "{} proxy={proxy_for:?}",
                    pair.constructor
                );
                if let Some(real) = proxy_for {
                    let call = runtime
                        .core()
                        .decode_spec(
                            &rtao_core::runtime::TypeSpec::Call,
                            plan.call().as_bytes(),
                            true,
                        )
                        .unwrap();
                    assert_eq!(
                        call.field("call_module").and_then(ValueExt::as_str),
                        Some("Proxy")
                    );
                    let expected = rtao_client::compose::proxy(
                        runtime,
                        real,
                        None,
                        pair.intent
                            .build_dyn(&BuildCx {
                                view: &snapshot as &dyn DynView,
                                origin: OriginView::Account(real),
                                runtime,
                                clock: &SystemClock,
                                spec: pair.intent.spec(),
                            })
                            .await
                            .unwrap()
                            .call,
                    )
                    .unwrap();
                    assert_eq!(plan.call(), &expected);
                    assert!(plan.call().as_bytes().ends_with(&expected_inner));
                    assert_eq!(plan.extras["proxy_for"], json!(real));
                } else {
                    assert_eq!(plan.call().as_bytes(), expected_inner);
                }
            }
        }
    }
    assert_eq!(seen.len(), 5);
    assert!(fake.state().lock().unwrap().submissions.is_empty());
    executor.client().close().await;
}

#[tokio::test]
async fn trusted_m2_policy_pairs_build_identical_chain_calls() {
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None).unwrap()).unwrap();
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    let fake = rtao_testkit::FakeChain::from_state(rtao_testkit::FakeState::default());
    let client = Client::from_rpc(rtao_testkit::rpc::RpcClient::new(fake.clone()), config)
        .await
        .unwrap();
    let snapshot = client.at(BlockRef::Finalized).await.unwrap();
    let runtime = &snapshot.block().runtime;
    for pair in m2_pairs() {
        let cx = BuildCx {
            view: &snapshot as &dyn DynView,
            origin: OriginView::Account(AccountId([43; 32])),
            runtime,
            clock: &SystemClock,
            spec: pair.intent.spec(),
        };
        let built = pair.intent.build_dyn(&cx).await.unwrap();
        let expected = runtime
            .core()
            .compose_call(pair.core.pallet(), pair.core.function(), pair.core.params())
            .unwrap();
        assert_eq!(
            built.call.as_bytes(),
            expected,
            "{} {:?}",
            pair.constructor,
            pair.intent.to_json_dyn()
        );
    }
    assert!(fake.state().lock().unwrap().submissions.is_empty());
    client.close().await;
}

#[test]
fn fee_caps_fail_closed_and_report_python_diagnostics_in_order() {
    let intent = Transfer {
        dest_ss58: DESTINATION.into(),
        amount_tao: Balance::<Tao>::from_rao(11).into(),
        keep_alive: true,
    };
    let policy = Policy {
        max_fee: Some(Balance::<Tao>::from_rao(10)),
        max_spend: Some(Balance::<Tao>::from_rao(10)),
        ..Default::default()
    };
    assert_eq!(
        policy.check(&intent, None),
        [
            "max_fee_tao τ0.000000010 is configured but the fee could not be estimated",
            "spend τ0.000000011 exceeds max_spend_tao τ0.000000010",
        ]
    );
    assert_eq!(
        policy.check(&intent, Some(Balance::<Tao>::from_rao(11))),
        [
            "fee τ0.000000011 exceeds max_fee_tao τ0.000000010",
            "spend τ0.000000011 exceeds max_spend_tao τ0.000000010",
        ]
    );
    let exact = Transfer {
        amount_tao: Balance::<Tao>::from_rao(10).into(),
        ..intent
    };
    assert!(
        policy
            .check(&exact, Some(Balance::<Tao>::from_rao(10)))
            .is_empty()
    );
    assert!(Policy::default().check(&exact, None).is_empty());
}

#[test]
fn one_rao_over_caps_is_rejected_through_the_native_maximum() {
    for amount in AMOUNTS.into_iter().filter(|amount| *amount > 0) {
        let intent = Transfer {
            dest_ss58: DESTINATION.into(),
            amount_tao: Balance::<Tao>::from_rao(amount).into(),
            keep_alive: true,
        };
        let exact = Policy {
            max_fee: Some(Balance::<Tao>::from_rao(amount)),
            max_spend: Some(Balance::<Tao>::from_rao(amount)),
            ..Default::default()
        };
        assert!(
            exact
                .check(&intent, Some(Balance::<Tao>::from_rao(amount)))
                .is_empty()
        );
        let too_small = Policy {
            max_fee: Some(Balance::<Tao>::from_rao(amount.checked_sub(1).unwrap())),
            max_spend: Some(Balance::<Tao>::from_rao(amount.checked_sub(1).unwrap())),
            ..Default::default()
        };
        assert_eq!(
            categories(&too_small.check(&intent, Some(Balance::<Tao>::from_rao(amount)))),
            [Violation::FeeExceeded, Violation::SpendExceeded],
            "amount={amount}"
        );
    }
}

#[test]
fn full_balance_sweeps_are_blocked_by_every_configured_native_spend_cap() {
    for keep_alive in [true, false] {
        let explicit = TransferAll {
            dest_ss58: DESTINATION.into(),
            keep_alive,
        };
        let alias = Transfer {
            dest_ss58: DESTINATION.into(),
            amount_tao: Amount::All,
            keep_alive,
        };
        for max in [0, 1, u64::MAX] {
            let policy = Policy {
                max_spend: Some(Balance::<Tao>::from_rao(max)),
                ..Default::default()
            };
            assert_eq!(
                policy.check(&explicit, None),
                [format!(
                    "transfer_all spends an amount that cannot be bounded before execution; max_spend_tao {} blocks it",
                    DynBalance::from(Balance::<Tao>::from_rao(max))
                )]
            );
            assert_eq!(
                categories(&policy.check(&alias, None)),
                [Violation::SpendUnbounded]
            );
        }
    }
}

#[test]
fn raw_call_submission_requires_explicit_policy_opt_in() {
    let policy = Policy {
        max_fee: Some(Balance::<Tao>::ZERO),
        max_spend: Some(Balance::<Tao>::ZERO),
        allowed_netuids: Some(BTreeSet::new()),
        ..Default::default()
    };
    assert_eq!(
        policy.check_raw_call(),
        ["raw call submission is disabled by policy (set allow_raw_calls=True)"]
    );
    assert!(
        Policy {
            allow_raw_calls: true,
            ..policy
        }
        .check_raw_call()
        .is_empty()
    );
}

#[test]
fn transfer_unit_mismatch_cannot_become_a_bounded_tao_spend() {
    let intent = Transfer {
        dest_ss58: DESTINATION.into(),
        amount_tao: Balance::<Alpha>::new(NetUid(7), 1).unwrap().into(),
        keep_alive: true,
    };
    assert!(matches!(
        intent.validate(),
        Err(IntentError::UnitMismatch(UnitError::UnitMismatch {
            expected: NetUid::ROOT,
            actual: NetUid(7)
        }))
    ));
    assert_eq!(Intent::spend(&intent), Spend::Unbounded);
    assert_eq!(
        categories(
            &Policy {
                max_spend: Some(Balance::<Tao>::from_rao(u64::MAX)),
                ..Default::default()
            }
            .check(&intent, None)
        ),
        [Violation::SpendUnbounded]
    );
    assert_eq!(Balance::<Tao>::try_from(-1_i128), Err(UnitError::Negative));
    assert_eq!(
        Balance::<Tao>::try_from(u128::from(u64::MAX).checked_add(1).unwrap()),
        Err(UnitError::Overflow)
    );
}

// The explicit M5-03 inventory is the same 21-call inventory as
// rtao-testkit/tests/core_oracle.rs, excluding retired set_root_claim_type.
const SUPPORTED_CORE_CONSTRUCTORS: [&str; 21] = [
    "transfer",
    "fund_evm_key",
    "transfer_allow_death",
    "transfer_all",
    "add_stake",
    "add_stake_limit",
    "remove_stake",
    "remove_stake_limit",
    "register_subnet",
    "start_call",
    "burned_register",
    "root_register",
    "move_stake",
    "move_stake_limit",
    "swap_stake",
    "transfer_stake",
    "transfer_stake_and_hotkey",
    "unstake_all",
    "unstake_all_alpha",
    "set_hyperparameter",
    "set_take",
];
const M5_SPANS: [(u16, u16); 6] = [
    (0, 1),
    (1, 0),
    (63, u16::MAX),
    (u16::MAX, 63),
    (63, 63),
    (0, 0),
];
const M5_PRICE: u64 = 1_000_000_000;
const POSITION_RAO: u64 = 5_000_000_003;
const FREE_RAO: u64 = 1_000_500_100;
const STAKEABLE_RAO: u64 = 1_000_000_000;
const OTHER_HOTKEY: AccountId = AccountId([45; 32]);
const DELEGATES_DEFAULT_TAKE: u16 = 11796;
const TAKE_STATES: [(AccountId, Option<u16>); 5] = [
    (DESTINATION, Some(32768)),
    (AccountId([46; 32]), None),
    (AccountId([47; 32]), Some(0)),
    (AccountId([48; 32]), Some(1)),
    (AccountId([50; 32]), Some(u16::MAX)),
];
const TAKE_TARGETS: [u16; 5] = [0, 1, DELEGATES_DEFAULT_TAKE, 32768, u16::MAX];
// Independent pinned Python test_evm.py::H160_TO_SS58_VECTORS.
const EVM_H160: &str = "0x1111111111111111111111111111111111111111";
const EVM_MIRROR: &str = "5DDYKUgyHe2Sx8a9oTZPAXrgFaTiGnYZXrecNdeC9bG7TbtX";
const EVM_MIRROR_KEY: [u8; 32] = [
    0x32, 0xf9, 0x55, 0xc9, 0x58, 0xe5, 0x11, 0x89, 0xa4, 0x92, 0x1a, 0xed, 0x41, 0xef, 0x00, 0x81,
    0x8f, 0x73, 0x68, 0xdf, 0xae, 0xc8, 0xd9, 0x96, 0x9f, 0x09, 0x10, 0x06, 0xf8, 0x06, 0x62, 0x28,
];

struct M5Pair {
    pair: Pair,
    bytes: Vec<u8>,
    scope: Vec<u16>,
    spend: Spend,
    global: bool,
}

fn position_amount(netuid: u16, rao: u64) -> Amount {
    if netuid == 0 {
        Balance::<Tao>::from_rao(rao).into()
    } else {
        Balance::<Alpha>::new(NetUid(netuid), rao).unwrap().into()
    }
}

fn m5_pair(
    constructor: &'static str,
    core: IntentCall,
    intent: impl DynIntent + 'static,
    bytes: Vec<u8>,
    scope: Vec<u16>,
    spend: Spend,
    global: bool,
) -> M5Pair {
    M5Pair {
        pair: Pair {
            constructor,
            core,
            intent: Box::new(intent),
        },
        bytes,
        scope,
        spend,
        global,
    }
}

// Fixed SCALE layouts from the pinned V478 metadata: explicit AccountId32,
// u16 netuids and u64 balances/prices, without runtime builder calls. These
// independently catch wrong function choice, field order, truncation and units.
fn stake_bytes(call: u8, netuid: u16, amount: u64, limit: Option<(u64, bool)>) -> Vec<u8> {
    let mut bytes = vec![7, call];
    bytes.extend_from_slice(DESTINATION.as_bytes());
    bytes.extend_from_slice(&netuid.to_le_bytes());
    bytes.extend_from_slice(&amount.to_le_bytes());
    if let Some((price, partial)) = limit {
        bytes.extend_from_slice(&price.to_le_bytes());
        bytes.push(u8::from(partial));
    }
    bytes
}

fn position_bytes(
    call: u8,
    origin: u16,
    destination: u16,
    amount: u64,
    limit: Option<u64>,
) -> Vec<u8> {
    let mut bytes = vec![7, call];
    match call {
        85 | 149 => {
            bytes.extend_from_slice(DESTINATION.as_bytes());
            bytes.extend_from_slice(OTHER_HOTKEY.as_bytes());
        }
        87 => bytes.extend_from_slice(DESTINATION.as_bytes()),
        86 => {
            bytes.extend_from_slice(DELEGATE.as_bytes());
            bytes.extend_from_slice(DESTINATION.as_bytes());
        }
        143 => {
            bytes.extend_from_slice(DELEGATE.as_bytes());
            bytes.extend_from_slice(DESTINATION.as_bytes());
            bytes.extend_from_slice(OTHER_HOTKEY.as_bytes());
        }
        _ => panic!("unknown fixed staking layout {call}"),
    }
    bytes.extend_from_slice(&origin.to_le_bytes());
    bytes.extend_from_slice(&destination.to_le_bytes());
    bytes.extend_from_slice(&amount.to_le_bytes());
    if let Some(price) = limit {
        bytes.extend_from_slice(&price.to_le_bytes());
        bytes.push(0);
    }
    bytes
}

fn m5_static_pairs() -> Vec<M5Pair> {
    let zero_tolerance = Ratio::new(0.0).unwrap();
    let mut pairs = vec![];
    for amount in AMOUNTS {
        let mut bytes = vec![5, 3, 0];
        bytes.extend_from_slice(&EVM_MIRROR_KEY);
        bytes.extend_from_slice(&compact_balance(amount));
        pairs.push(m5_pair(
            "fund_evm_key",
            IntentCall::fund_evm_key(EVM_MIRROR, u128::from(amount)),
            FundEvmKey {
                evm_address: EVM_H160.into(),
                amount_tao: Balance::<Tao>::from_rao(amount).into(),
            },
            bytes,
            vec![],
            Spend::Bounded(Balance::<Tao>::from_rao(amount)),
            false,
        ));
        for netuid in M4_NETUIDS {
            pairs.push(m5_pair(
                "add_stake",
                IntentCall::add_stake(DESTINATION.to_string(), netuid, u128::from(amount)),
                AddStake {
                    hotkey_ss58: DESTINATION.into(),
                    netuid: NetUid(netuid),
                    amount_tao: Balance::<Tao>::from_rao(amount).into(),
                    slippage_protection: false,
                    rate_tolerance: zero_tolerance,
                },
                stake_bytes(2, netuid, amount, None),
                vec![netuid],
                Spend::Bounded(Balance::<Tao>::from_rao(amount)),
                false,
            ));
            pairs.push(m5_pair(
                "add_stake_limit",
                IntentCall::add_stake_limit(
                    DESTINATION.to_string(),
                    netuid,
                    u128::from(amount),
                    u128::from(M5_PRICE),
                    false,
                ),
                AddStakeLimit {
                    hotkey_ss58: DESTINATION.into(),
                    netuid: NetUid(netuid),
                    amount_tao: Balance::<Tao>::from_rao(amount).into(),
                    limit_price_rao: M5_PRICE,
                    allow_partial: false,
                },
                stake_bytes(88, netuid, amount, Some((M5_PRICE, false))),
                vec![netuid],
                Spend::Bounded(Balance::<Tao>::from_rao(amount)),
                false,
            ));
            pairs.push(m5_pair(
                "remove_stake",
                IntentCall::remove_stake(DESTINATION.to_string(), netuid, u128::from(amount)),
                RemoveStake {
                    hotkey_ss58: DESTINATION.into(),
                    netuid: NetUid(netuid),
                    amount_alpha: position_amount(netuid, amount),
                    slippage_protection: false,
                    rate_tolerance: zero_tolerance,
                    claim: false,
                },
                stake_bytes(3, netuid, amount, None),
                vec![netuid],
                Spend::None,
                false,
            ));
            pairs.push(m5_pair(
                "remove_stake_limit",
                IntentCall::remove_stake_limit(
                    DESTINATION.to_string(),
                    netuid,
                    u128::from(amount),
                    u128::from(M5_PRICE),
                    false,
                ),
                RemoveStakeLimit {
                    hotkey_ss58: DESTINATION.into(),
                    netuid: NetUid(netuid),
                    amount_alpha: position_amount(netuid, amount),
                    limit_price_rao: M5_PRICE,
                    allow_partial: false,
                    claim: false,
                },
                stake_bytes(89, netuid, amount, Some((M5_PRICE, false))),
                vec![netuid],
                Spend::None,
                false,
            ));
        }
        for (origin, destination) in M5_SPANS {
            pairs.push(m5_pair(
                "move_stake",
                IntentCall::move_stake(
                    DESTINATION.to_string(),
                    origin,
                    OTHER_HOTKEY.to_string(),
                    destination,
                    u128::from(amount),
                ),
                MoveStake {
                    origin_hotkey_ss58: DESTINATION.into(),
                    origin_netuid: NetUid(origin),
                    dest_hotkey_ss58: OTHER_HOTKEY.into(),
                    dest_netuid: NetUid(destination),
                    amount_alpha: position_amount(origin, amount),
                    slippage_protection: false,
                    rate_tolerance: zero_tolerance,
                    claim: false,
                },
                position_bytes(85, origin, destination, amount, None),
                vec![origin, destination],
                Spend::None,
                false,
            ));
            // Python selects the single protected move call only across subnets.
            if origin != destination {
                pairs.push(m5_pair(
                    "move_stake_limit",
                    IntentCall::move_stake_limit(
                        DESTINATION.to_string(),
                        origin,
                        OTHER_HOTKEY.to_string(),
                        destination,
                        u128::from(amount),
                        u128::from(M5_PRICE),
                        false,
                    ),
                    MoveStake {
                        origin_hotkey_ss58: DESTINATION.into(),
                        origin_netuid: NetUid(origin),
                        dest_hotkey_ss58: OTHER_HOTKEY.into(),
                        dest_netuid: NetUid(destination),
                        amount_alpha: position_amount(origin, amount),
                        slippage_protection: true,
                        rate_tolerance: zero_tolerance,
                        claim: false,
                    },
                    position_bytes(149, origin, destination, amount, Some(M5_PRICE)),
                    vec![origin, destination],
                    Spend::None,
                    false,
                ));
            }
            pairs.push(m5_pair(
                "swap_stake",
                IntentCall::swap_stake(
                    DESTINATION.to_string(),
                    origin,
                    destination,
                    u128::from(amount),
                ),
                SwapStake {
                    hotkey_ss58: DESTINATION.into(),
                    origin_netuid: NetUid(origin),
                    dest_netuid: NetUid(destination),
                    amount_alpha: position_amount(origin, amount),
                    slippage_protection: false,
                    rate_tolerance: zero_tolerance,
                },
                position_bytes(87, origin, destination, amount, None),
                vec![origin, destination],
                Spend::None,
                false,
            ));
            for landing in [None, Some(OTHER_HOTKEY)] {
                let (name, core, index) = if let Some(hotkey) = landing {
                    (
                        "transfer_stake_and_hotkey",
                        IntentCall::transfer_stake_and_hotkey(
                            DELEGATE.to_string(),
                            DESTINATION.to_string(),
                            hotkey.to_string(),
                            origin,
                            destination,
                            u128::from(amount),
                        ),
                        143,
                    )
                } else {
                    (
                        "transfer_stake",
                        IntentCall::transfer_stake(
                            DELEGATE.to_string(),
                            DESTINATION.to_string(),
                            origin,
                            destination,
                            u128::from(amount),
                        ),
                        86,
                    )
                };
                pairs.push(m5_pair(
                    name,
                    core,
                    TransferStake {
                        dest_coldkey_ss58: DELEGATE.into(),
                        hotkey_ss58: DESTINATION.into(),
                        origin_netuid: NetUid(origin),
                        dest_netuid: NetUid(destination),
                        amount_alpha: position_amount(origin, amount),
                        dest_hotkey_ss58: landing.map(Into::into),
                    },
                    position_bytes(index, origin, destination, amount, None),
                    vec![origin, destination],
                    Spend::Unbounded,
                    false,
                ));
            }
        }
    }
    for (name, core, intent, index) in [
        (
            "unstake_all",
            IntentCall::unstake_all(DESTINATION.to_string()),
            Box::new(UnstakeAll {
                hotkey_ss58: DESTINATION.into(),
                claim: false,
            }) as Box<dyn DynIntent>,
            83,
        ),
        (
            "unstake_all_alpha",
            IntentCall::unstake_all_alpha(DESTINATION.to_string()),
            Box::new(UnstakeAllAlpha {
                hotkey_ss58: DESTINATION.into(),
            }) as Box<dyn DynIntent>,
            84,
        ),
    ] {
        let mut bytes = vec![7, index];
        bytes.extend_from_slice(DESTINATION.as_bytes());
        pairs.push(M5Pair {
            pair: Pair {
                constructor: name,
                core,
                intent,
            },
            bytes,
            scope: vec![],
            spend: Spend::None,
            global: true,
        });
    }
    // Compact amount mode boundaries have explicit byte vectors as well as
    // core-constructor comparisons; they do not depend on compact_balance().
    for (amount, compact) in [
        (63, vec![252]),
        (64, vec![1, 1]),
        (16_383, vec![253, 255]),
        (16_384, vec![2, 0, 1, 0]),
        (1 << 30, vec![3, 0, 0, 0, 64]),
    ] {
        let mut bytes = vec![5, 3, 0];
        bytes.extend_from_slice(&EVM_MIRROR_KEY);
        bytes.extend(compact);
        pairs.push(m5_pair(
            "fund_evm_key",
            IntentCall::fund_evm_key(EVM_MIRROR, u128::from(amount)),
            FundEvmKey {
                evm_address: EVM_H160.into(),
                amount_tao: Balance::<Tao>::from_rao(amount).into(),
            },
            bytes,
            vec![],
            Spend::Bounded(Balance::<Tao>::from_rao(amount)),
            false,
        ));
    }
    pairs
}

fn compact_balance(value: u64) -> Vec<u8> {
    if value < 64 {
        vec![(value as u8) << 2]
    } else if value < 16384 {
        ((value as u16) << 2 | 1).to_le_bytes().to_vec()
    } else if value < 1 << 30 {
        ((value as u32) << 2 | 2).to_le_bytes().to_vec()
    } else {
        let bytes = value.to_le_bytes();
        let count = (8usize)
            .saturating_sub((value.leading_zeros() / 8) as usize)
            .max(4);
        let mut encoded = vec![((count - 4) as u8) << 2 | 3];
        encoded.extend_from_slice(&bytes[..count]);
        encoded
    }
}

async fn m5_fixture() -> (Client, rtao_testkit::FakeChain) {
    let mut state = rtao_testkit::FakeState::default();
    state
        .seed_constant("Balances", "ExistentialDeposit", json!(100))
        .unwrap();
    state
        .seed_python(
            "System",
            "Account",
            &[json!(OWNER.to_string())],
            json!({"data":{"free":FREE_RAO}}),
        )
        .unwrap();
    state
        .seed_runtime("SwapRuntimeApi", "current_alpha_price", json!(M5_PRICE))
        .unwrap();
    state
        .seed_runtime(
            "StakeInfoRuntimeApi",
            "get_stake_info_for_hotkey_coldkey_netuid",
            json!({"stake":POSITION_RAO}),
        )
        .unwrap();
    for (hotkey, current) in TAKE_STATES {
        if let Some(current) = current {
            state
                .seed(
                    &rtao_runtime::storage::subtensor_module::DELEGATES,
                    (hotkey,),
                    PerU16(current),
                )
                .unwrap();
        } else {
            let entry = state
                .runtime
                .storage_entry("SubtensorModule", "Delegates")
                .unwrap();
            assert_eq!(entry.default_bytes, DELEGATES_DEFAULT_TAKE.to_le_bytes());
            let key = state
                .runtime
                .storage_key(entry, &[rtao_core::codec::Value::Bytes(hotkey.0.to_vec())])
                .unwrap();
            state.seed_encoded(key, None);
        }
    }
    let fake = rtao_testkit::FakeChain::from_state(state);
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None).unwrap()).unwrap();
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    let client = Client::from_rpc(rtao_testkit::rpc::RpcClient::new(fake.clone()), config)
        .await
        .unwrap();
    (client, fake)
}

// Ordinary bounded loopback HTTP exists only inside this test binary. It serves
// the same encoded FakeState as the async client; no node, Docker, Python or
// deployed service is involved. The real core set_take must do its own read.
struct CoreHttp {
    endpoint: String,
    stop: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<Result<(), String>>>,
}

impl CoreHttp {
    async fn start(fake: rtao_testkit::FakeChain) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let (stop, mut stopped) = oneshot::channel();
        let task = tokio::spawn(async move {
            let mut count = 0usize;
            loop {
                tokio::select! {
                    _=&mut stopped=>return Ok(()),
                    accepted=tokio::time::timeout(Duration::from_secs(30),listener.accept())=>{
                        let (stream,_)=accepted.map_err(|error|error.to_string())?.map_err(|error|error.to_string())?;
                        count+=1;
                        if count>64 {return Err("core constructor fixture exceeded 64 HTTP requests".into());}
                        tokio::time::timeout(Duration::from_secs(10),core_http_request(stream,&fake)).await.map_err(|error|error.to_string())??;
                    }
                }
            }
        });
        Self {
            endpoint,
            stop: Some(stop),
            task: Some(task),
        }
    }
    async fn finish(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        self.task.take().unwrap().await.unwrap().unwrap();
    }
}

impl Drop for CoreHttp {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

async fn core_http_request(
    mut stream: TcpStream,
    fake: &rtao_testkit::FakeChain,
) -> Result<(), String> {
    const MAX_HEADER: usize = 8192;
    const MAX_BODY: usize = 65536;
    let mut request = Vec::new();
    let mut chunk = [0u8; 1024];
    let header_end = loop {
        let read = stream
            .read(&mut chunk)
            .await
            .map_err(|error| error.to_string())?;
        if read == 0 {
            return Err("core HTTP request ended before headers".into());
        }
        request.extend_from_slice(&chunk[..read]);
        if let Some(end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
            if end + 4 > MAX_HEADER {
                return Err("core HTTP headers exceeded limit".into());
            }
            break end + 4;
        }
        if request.len() > MAX_HEADER {
            return Err("core HTTP headers exceeded limit".into());
        }
    };
    let header = std::str::from_utf8(&request[..header_end]).map_err(|error| error.to_string())?;
    if !header.starts_with("POST / HTTP/1.1\r\n") {
        return Err("expected bounded core JSON-RPC POST".into());
    }
    let mut length = None;
    for line in header.lines().skip(1) {
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                if length.is_some() {
                    return Err("duplicate HTTP Content-Length".into());
                }
                length = Some(
                    value
                        .trim()
                        .parse::<usize>()
                        .map_err(|error| error.to_string())?,
                );
            }
            if name.eq_ignore_ascii_case("transfer-encoding") {
                return Err("chunked fixture requests are unsupported".into());
            }
        }
    }
    let length = length.ok_or("core POST omitted Content-Length")?;
    if length > MAX_BODY {
        return Err("core HTTP body exceeded limit".into());
    }
    while request.len() < header_end + length {
        let remaining = header_end + length - request.len();
        let read = stream
            .read(&mut chunk[..remaining.min(1024)])
            .await
            .map_err(|error| error.to_string())?;
        if read == 0 {
            return Err("core HTTP request body was truncated".into());
        }
        request.extend_from_slice(&chunk[..read]);
    }
    if request.len() != header_end + length {
        return Err("unexpected trailing core HTTP request bytes".into());
    }
    let request: rtao_testkit::RpcRequest =
        serde_json::from_slice(&request[header_end..]).map_err(|error| error.to_string())?;
    if !matches!(
        request.method.as_str(),
        "system_properties"
            | "state_getRuntimeVersion"
            | "state_call"
            | "state_getMetadata"
            | "chain_getBlockHash"
            | "state_getStorage"
    ) {
        return Err(format!(
            "unexpected constructor oracle RPC {}",
            request.method
        ));
    }
    let reply = fake.mutate_state(|state| state.dispatch(&request));
    if !reply.notifications.is_empty() {
        return Err("constructor unexpectedly requested subscriptions".into());
    }
    let body = match reply
        .outcome
        .ok_or("constructor fixture must answer every request")?
    {
        Ok(result) => json!({"jsonrpc":"2.0","id":request.id,"result":result}),
        Err(error) => {
            json!({"jsonrpc":"2.0","id":request.id,"error":{"code":error.code,"message":error.message,"data":error.data}})
        }
    };
    let body = serde_json::to_vec(&body).map_err(|error| error.to_string())?;
    if body.len() > 8 * 1024 * 1024 {
        return Err("core HTTP response exceeded 8 MiB fixture limit".into());
    }
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream
        .write_all(header.as_bytes())
        .await
        .map_err(|error| error.to_string())?;
    stream
        .write_all(&body)
        .await
        .map_err(|error| error.to_string())?;
    stream.shutdown().await.map_err(|error| error.to_string())
}

async fn m5_pairs(fake: &rtao_testkit::FakeChain) -> Vec<M5Pair> {
    let mut pairs = m5_static_pairs();
    let requests_before = fake.requests().len();
    let server = CoreHttp::start(fake.clone()).await;
    let endpoint = server.endpoint.clone();
    let takes = tokio::task::spawn_blocking(move || {
        // The blocking SDK is an oracle only. Construct/drop it on the blocking
        // worker, never inside Tokio's async runtime.
        #[allow(clippy::disallowed_methods)]
        let core = rtao_testkit::core_client::Client::connect(endpoint).unwrap();
        let mut pairs = vec![];
        for (hotkey, current) in TAKE_STATES {
            for target in TAKE_TARGETS {
                let constructor = IntentCall::set_take(&core, hotkey.to_string(), target);
                if target == current.unwrap_or(DELEGATES_DEFAULT_TAKE) {
                    assert_eq!(
                        constructor.unwrap_err().to_string(),
                        format!("policy error: delegate take is already {target}/65535; nothing to change")
                    );
                    continue;
                }
                let function = if target < current.unwrap_or(DELEGATES_DEFAULT_TAKE) {
                    65
                } else {
                    66
                };
                for explicit in if hotkey == DESTINATION {
                    vec![false, true]
                } else {
                    vec![true]
                } {
                    let mut bytes = vec![7, function];
                    bytes.extend_from_slice(hotkey.as_bytes());
                    bytes.extend_from_slice(&target.to_le_bytes());
                    pairs.push(m5_pair(
                        "set_take",
                        constructor.as_ref().unwrap().clone(),
                        SetTake {
                            take: json!(target),
                            hotkey_ss58: explicit.then(|| hotkey.into()),
                        },
                        bytes,
                        vec![],
                        Spend::None,
                        false,
                    ));
                }
            }
        }
        pairs
    })
    .await
    .unwrap();
    server.finish().await;
    let requests = fake.requests();
    let reads = requests[requests_before..]
        .iter()
        .filter(|request| request.method == "state_getStorage")
        .collect::<Vec<_>>();
    assert_eq!(reads.len(), TAKE_STATES.len() * TAKE_TARGETS.len());
    // One actual Delegates RPC per constructor invocation, including no-ops;
    // neither fabricated IntentCall metadata nor a substituted client is used.
    for ((hotkey, _), group) in TAKE_STATES
        .into_iter()
        .zip(reads.chunks(TAKE_TARGETS.len()))
    {
        let key = fake.mutate_state(|state| {
            let entry = state
                .runtime
                .storage_entry("SubtensorModule", "Delegates")
                .unwrap();
            state
                .runtime
                .storage_key(entry, &[rtao_core::codec::Value::Bytes(hotkey.0.to_vec())])
                .unwrap()
        });
        let key = format!("0x{}", hex::encode(key));
        for request in group {
            assert_eq!(request.params, json!([key]));
        }
    }
    pairs.extend(takes);
    pairs
}

fn category_parts(messages: &[String]) -> (Vec<Violation>, Vec<u16>) {
    let mut prefix = vec![];
    let mut netuids = vec![];
    for category in categories(messages) {
        match category {
            Violation::Netuid(netuid) => netuids.push(netuid),
            other => {
                assert!(
                    netuids.is_empty(),
                    "subnet diagnostics must follow fee/spend/global checks"
                );
                prefix.push(other);
            }
        }
    }
    (prefix, netuids)
}

fn compare_m5_policy(pair: &M5Pair, policy: &Policy, fee: Option<u64>) {
    let core = core_policy(policy).check(&pair.pair.core, fee.map(u128::from));
    let actual = policy.check(
        pair.pair.intent.semantic_intent_dyn(),
        fee.map(Balance::<Tao>::from_rao),
    );
    let (core_prefix, core_scope) = category_parts(&core);
    let (prefix, scope) = category_parts(&actual);
    assert_eq!(
        prefix,
        core_prefix,
        "{} {:?}, policy={policy:?}, fee={fee:?}",
        pair.pair.constructor,
        pair.pair.intent.to_json_dyn()
    );
    let missing = pair
        .scope
        .iter()
        .copied()
        .filter(|netuid| {
            policy
                .allowed_netuids
                .as_ref()
                .is_some_and(|allowed| !allowed.contains(&NetUid(*netuid)))
        })
        .collect::<Vec<_>>();
    assert_eq!(
        scope, missing,
        "Python origin/destination order and duplicates: {}",
        pair.pair.constructor
    );
    assert_eq!(
        core_scope,
        missing
            .into_iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>(),
        "core canonicalizes scope: {}",
        pair.pair.constructor
    );
    assert_eq!(actual.is_empty(), core.is_empty());
}

#[tokio::test]
async fn all_twenty_one_real_core_constructors_match_native_policy_categories_and_scope_sets() {
    let (client, fake) = m5_fixture().await;
    let pairs = m5_pairs(&fake).await;
    let mut seen = m2_pairs()
        .into_iter()
        .chain(m4_pairs())
        .map(|pair| pair.constructor)
        .collect::<BTreeSet<_>>();
    seen.extend(pairs.iter().map(|pair| pair.pair.constructor));
    assert_eq!(seen, SUPPORTED_CORE_CONSTRUCTORS.into_iter().collect());
    let allowlists = [
        None,
        Some(BTreeSet::new()),
        Some(BTreeSet::from([NetUid::ROOT])),
        Some(BTreeSet::from([NetUid(1), NetUid(63)])),
        Some(M4_NETUIDS.map(NetUid).into_iter().collect()),
    ];
    let mut comparisons = 0usize;
    for pair in &pairs {
        pair.pair.intent.validate_dyn().unwrap();
        assert_eq!(pair.pair.core.signer_role(), CoreSignerRole::Coldkey);
        assert_eq!(
            pair.pair.intent.signer_role_dyn().unwrap(),
            SignerRole::Coldkey
        );
        assert_eq!(pair.pair.intent.spend_dyn(), pair.spend);
        assert_eq!(
            pair.pair.intent.touches_netuids_dyn().as_slice(),
            pair.scope
        );
        assert_eq!(pair.pair.intent.affects_all_subnets_dyn(), pair.global);
        for max_fee in CAPS {
            for max_spend in CAPS {
                for allowed in &allowlists {
                    for allow_raw_calls in [false, true] {
                        for fee in FEES {
                            compare_m5_policy(
                                pair,
                                &Policy {
                                    max_fee: max_fee.map(Balance::<Tao>::from_rao),
                                    max_spend: max_spend.map(Balance::<Tao>::from_rao),
                                    allowed_netuids: allowed.clone(),
                                    allow_raw_calls,
                                },
                                fee,
                            );
                            comparisons += 1;
                        }
                    }
                }
            }
        }
        let raw = pair.pair.core.clone().raw();
        for allow_raw_calls in [false, true] {
            let policy = Policy {
                allow_raw_calls,
                ..Default::default()
            };
            assert_eq!(
                categories(&policy.check_raw_call()),
                categories(&core_policy(&policy).check(&raw, None))
            );
        }
    }
    assert_eq!(
        comparisons,
        pairs.len() * CAPS.len() * CAPS.len() * allowlists.len() * 2 * FEES.len()
    );
    assert!(fake.mutate_state(|state| state.submissions.is_empty()));
    client.close().await;
}

#[tokio::test]
async fn trusted_m5_policy_pairs_build_both_core_and_independent_scale_bytes() {
    let (client, fake) = m5_fixture().await;
    let pairs = m5_pairs(&fake).await;
    let snapshot = client.at(BlockRef::Finalized).await.unwrap();
    let runtime = &snapshot.block().runtime;
    let keys = PublicKeys(OWNER);
    for pair in pairs {
        assert_m5_build(&pair, &snapshot, &keys).await;
    }
    for (hotkey, current) in TAKE_STATES {
        let intent = SetTake {
            take: json!(current.unwrap_or(DELEGATES_DEFAULT_TAKE)),
            hotkey_ss58: Some(hotkey.into()),
        };
        assert_eq!(
            intent
                .build(&BuildCx {
                    view: &snapshot,
                    origin: OriginView::Keys(&keys),
                    runtime,
                    clock: &SystemClock,
                    spec: <SetTake as rtao_tx::IntentInterface>::SPEC
                })
                .await
                .unwrap_err()
                .to_string(),
            format!(
                "delegate take is already {}/65535; nothing to change",
                current.unwrap_or(DELEGATES_DEFAULT_TAKE)
            )
        );
    }
    assert!(fake.mutate_state(|state| state.submissions.is_empty()));
    client.close().await;
}

async fn assert_m5_build(pair: &M5Pair, snapshot: &rtao_client::Snapshot, keys: &PublicKeys) {
    let runtime = &snapshot.block().runtime;
    pair.pair.intent.validate_dyn().unwrap();
    let built = pair
        .pair
        .intent
        .build_dyn(&BuildCx {
            view: snapshot as &dyn DynView,
            origin: OriginView::Keys(keys),
            runtime,
            clock: &SystemClock,
            spec: pair.pair.intent.spec(),
        })
        .await
        .unwrap();
    let expected = runtime
        .core()
        .compose_call(
            pair.pair.core.pallet(),
            pair.pair.core.function(),
            pair.pair.core.params(),
        )
        .unwrap();
    assert_eq!(
        built.call.as_bytes(),
        expected,
        "{} {:?}",
        pair.pair.constructor,
        pair.pair.intent.to_json_dyn()
    );
    assert_eq!(
        built.call.as_bytes(),
        pair.bytes,
        "independent V478 bytes: {} {:?}",
        pair.pair.constructor,
        pair.pair.intent.to_json_dyn()
    );
    assert_eq!(
        built.call,
        rtao_runtime::CallBytes::decode(&pair.bytes, runtime).unwrap()
    );
    let decoded = runtime
        .core()
        .decode_spec(
            &rtao_core::runtime::TypeSpec::Call,
            built.call.as_bytes(),
            true,
        )
        .unwrap();
    assert_eq!(
        decoded.field("call_module").and_then(ValueExt::as_str),
        Some(pair.pair.core.pallet())
    );
    assert_eq!(
        decoded.field("call_function").and_then(ValueExt::as_str),
        Some(pair.pair.core.function())
    );
    assert!(built.extras.is_empty());
}

#[test]
fn two_subnet_diagnostic_tails_keep_python_order_and_core_canonicalization() {
    let policy = Policy {
        max_fee: Some(Balance::<Tao>::ZERO),
        max_spend: Some(Balance::<Tao>::ZERO),
        allowed_netuids: Some(BTreeSet::new()),
        ..Default::default()
    };
    let pairs = m5_static_pairs();
    let transfer = pairs
        .iter()
        .find(|pair| pair.pair.constructor == "transfer_stake" && pair.scope == [u16::MAX, 63])
        .unwrap();
    let native = policy.check(
        transfer.pair.intent.semantic_intent_dyn(),
        Some(Balance::<Tao>::from_rao(1)),
    );
    let core = core_policy(&policy).check(&transfer.pair.core, Some(1));
    assert_eq!(
        categories(&native),
        [
            Violation::FeeExceeded,
            Violation::SpendUnbounded,
            Violation::Netuid(u16::MAX),
            Violation::Netuid(63)
        ]
    );
    assert_eq!(
        categories(&core),
        [
            Violation::FeeExceeded,
            Violation::SpendUnbounded,
            Violation::Netuid(63),
            Violation::Netuid(u16::MAX)
        ]
    );
    let repeated = pairs
        .iter()
        .find(|pair| pair.pair.constructor == "move_stake" && pair.scope == [0, 0])
        .unwrap();
    assert_eq!(
        categories(&policy.check(
            repeated.pair.intent.semantic_intent_dyn(),
            Some(Balance::<Tao>::from_rao(1))
        )),
        [
            Violation::FeeExceeded,
            Violation::Netuid(0),
            Violation::Netuid(0)
        ]
    );
    assert_eq!(
        categories(&core_policy(&policy).check(&repeated.pair.core, Some(1))),
        [Violation::FeeExceeded, Violation::Netuid(0)]
    );
}

#[tokio::test]
async fn explicit_limit_prices_and_partial_flags_keep_native_boundaries() {
    let (client, fake) = m5_fixture().await;
    let snapshot = client.at(BlockRef::Finalized).await.unwrap();
    let keys = PublicKeys(OWNER);
    for netuid in M4_NETUIDS {
        for amount in [0, 1, u64::MAX] {
            for limit in [0, 1, 9_007_199_254_740_993, u64::MAX] {
                for partial in [false, true] {
                    let add = m5_pair(
                        "add_stake_limit",
                        IntentCall::add_stake_limit(
                            DESTINATION.to_string(),
                            netuid,
                            u128::from(amount),
                            u128::from(limit),
                            partial,
                        ),
                        AddStakeLimit {
                            hotkey_ss58: DESTINATION.into(),
                            netuid: NetUid(netuid),
                            amount_tao: Balance::<Tao>::from_rao(amount).into(),
                            limit_price_rao: limit,
                            allow_partial: partial,
                        },
                        stake_bytes(88, netuid, amount, Some((limit, partial))),
                        vec![netuid],
                        Spend::Bounded(Balance::<Tao>::from_rao(amount)),
                        false,
                    );
                    let remove = m5_pair(
                        "remove_stake_limit",
                        IntentCall::remove_stake_limit(
                            DESTINATION.to_string(),
                            netuid,
                            u128::from(amount),
                            u128::from(limit),
                            partial,
                        ),
                        RemoveStakeLimit {
                            hotkey_ss58: DESTINATION.into(),
                            netuid: NetUid(netuid),
                            amount_alpha: position_amount(netuid, amount),
                            limit_price_rao: limit,
                            allow_partial: partial,
                            claim: false,
                        },
                        stake_bytes(89, netuid, amount, Some((limit, partial))),
                        vec![netuid],
                        Spend::None,
                        false,
                    );
                    for pair in [add, remove] {
                        assert_m5_build(&pair, &snapshot, &keys).await;
                        for cap in [Some(0), Some(amount), Some(u64::MAX)] {
                            compare_m5_policy(
                                &pair,
                                &Policy {
                                    max_spend: cap.map(Balance::<Tao>::from_rao),
                                    allowed_netuids: Some(BTreeSet::new()),
                                    ..Default::default()
                                },
                                None,
                            );
                        }
                    }
                }
            }
        }
    }
    assert!(fake.mutate_state(|state| state.submissions.is_empty()));
    client.close().await;
}

fn m5_all_position_pairs() -> Vec<M5Pair> {
    let mut all = vec![];
    for mut pair in m5_static_pairs().into_iter().filter(|pair| {
        matches!(
            pair.pair.constructor,
            "remove_stake"
                | "remove_stake_limit"
                | "move_stake"
                | "move_stake_limit"
                | "swap_stake"
                | "transfer_stake"
                | "transfer_stake_and_hotkey"
        )
    }) {
        let amount = pair
            .pair
            .core
            .params()
            .field("amount_unstaked")
            .or_else(|| pair.pair.core.params().field("alpha_amount"))
            .and_then(ValueExt::as_u128)
            .unwrap();
        if amount != 1 {
            continue;
        }
        let origin = pair.scope[0];
        let destination = pair.scope.get(1).copied().unwrap_or(origin);
        let amount = u128::from(POSITION_RAO);
        let (core, bytes) = match pair.pair.constructor {
            "remove_stake" => (
                IntentCall::remove_stake(DESTINATION.to_string(), origin, amount),
                stake_bytes(3, origin, POSITION_RAO, None),
            ),
            "remove_stake_limit" => (
                IntentCall::remove_stake_limit(
                    DESTINATION.to_string(),
                    origin,
                    amount,
                    u128::from(M5_PRICE),
                    false,
                ),
                stake_bytes(89, origin, POSITION_RAO, Some((M5_PRICE, false))),
            ),
            "move_stake" => (
                IntentCall::move_stake(
                    DESTINATION.to_string(),
                    origin,
                    OTHER_HOTKEY.to_string(),
                    destination,
                    amount,
                ),
                position_bytes(85, origin, destination, POSITION_RAO, None),
            ),
            "move_stake_limit" => (
                IntentCall::move_stake_limit(
                    DESTINATION.to_string(),
                    origin,
                    OTHER_HOTKEY.to_string(),
                    destination,
                    amount,
                    u128::from(M5_PRICE),
                    false,
                ),
                position_bytes(149, origin, destination, POSITION_RAO, Some(M5_PRICE)),
            ),
            "swap_stake" => (
                IntentCall::swap_stake(DESTINATION.to_string(), origin, destination, amount),
                position_bytes(87, origin, destination, POSITION_RAO, None),
            ),
            "transfer_stake" => (
                IntentCall::transfer_stake(
                    DELEGATE.to_string(),
                    DESTINATION.to_string(),
                    origin,
                    destination,
                    amount,
                ),
                position_bytes(86, origin, destination, POSITION_RAO, None),
            ),
            "transfer_stake_and_hotkey" => (
                IntentCall::transfer_stake_and_hotkey(
                    DELEGATE.to_string(),
                    DESTINATION.to_string(),
                    OTHER_HOTKEY.to_string(),
                    origin,
                    destination,
                    amount,
                ),
                position_bytes(143, origin, destination, POSITION_RAO, None),
            ),
            name => panic!("unexpected position constructor {name}"),
        };
        let mut args = pair.pair.intent.to_json_dyn();
        args["amount_alpha"] = json!("all");
        pair.pair.intent = rtao_intents::AnyIntent::from_tool(pair.pair.intent.spec().name, args)
            .unwrap()
            .into_dyn();
        pair.pair.core = core;
        pair.bytes = bytes;
        all.push(pair);
    }
    all
}

#[tokio::test]
async fn all_position_inputs_resolve_state_and_keep_constructor_policy_scope() {
    let (client, fake) = m5_fixture().await;
    let snapshot = client.at(BlockRef::Finalized).await.unwrap();
    let keys = PublicKeys(OWNER);
    let pairs = m5_all_position_pairs();
    assert_eq!(
        pairs
            .iter()
            .map(|pair| pair.pair.constructor)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([
            "remove_stake",
            "remove_stake_limit",
            "move_stake",
            "move_stake_limit",
            "swap_stake",
            "transfer_stake",
            "transfer_stake_and_hotkey"
        ])
    );
    for pair in pairs {
        assert_m5_build(&pair, &snapshot, &keys).await;
        for cap in CAPS {
            for allowed in [
                None,
                Some(BTreeSet::new()),
                Some(M4_NETUIDS.map(NetUid).into_iter().collect()),
            ] {
                compare_m5_policy(
                    &pair,
                    &Policy {
                        max_spend: cap.map(Balance::<Tao>::from_rao),
                        allowed_netuids: allowed,
                        ..Default::default()
                    },
                    None,
                );
            }
        }
    }
    assert!(fake.mutate_state(|state| state.submissions.is_empty()));
    client.close().await;
}

#[tokio::test]
async fn all_free_tao_keeps_python_unbounded_spend_while_core_exact_constructor_is_bounded() {
    let (client, fake) = m5_fixture().await;
    let snapshot = client.at(BlockRef::Finalized).await.unwrap();
    let keys = PublicKeys(OWNER);
    let tolerance = Ratio::new(0.0).unwrap();
    for netuid in M4_NETUIDS {
        let plain = m5_pair(
            "add_stake",
            IntentCall::add_stake(DESTINATION.to_string(), netuid, u128::from(STAKEABLE_RAO)),
            AddStake {
                hotkey_ss58: DESTINATION.into(),
                netuid: NetUid(netuid),
                amount_tao: Amount::All,
                slippage_protection: false,
                rate_tolerance: tolerance,
            },
            stake_bytes(2, netuid, STAKEABLE_RAO, None),
            vec![netuid],
            Spend::Unbounded,
            false,
        );
        let limited = m5_pair(
            "add_stake_limit",
            IntentCall::add_stake_limit(
                DESTINATION.to_string(),
                netuid,
                u128::from(STAKEABLE_RAO),
                u128::from(M5_PRICE),
                false,
            ),
            AddStakeLimit {
                hotkey_ss58: DESTINATION.into(),
                netuid: NetUid(netuid),
                amount_tao: Amount::All,
                limit_price_rao: M5_PRICE,
                allow_partial: false,
            },
            stake_bytes(88, netuid, STAKEABLE_RAO, Some((M5_PRICE, false))),
            vec![netuid],
            Spend::Unbounded,
            false,
        );
        for pair in [plain, limited] {
            assert_m5_build(&pair, &snapshot, &keys).await;
            assert_eq!(pair.pair.intent.spend_dyn(), Spend::Unbounded);
            // The core has no "all" input on these two constructors. Do not
            // overwrite its trusted spend to manufacture equivalence: Python's
            // drain guard is deliberately stricter than this resolved amount.
            for cap in CAPS.into_iter().flatten() {
                let policy = Policy {
                    max_spend: Some(Balance::<Tao>::from_rao(cap)),
                    ..Default::default()
                };
                assert_eq!(
                    categories(&policy.check(pair.pair.intent.semantic_intent_dyn(), None)),
                    [Violation::SpendUnbounded]
                );
                let expected = if STAKEABLE_RAO > cap {
                    vec![Violation::SpendExceeded]
                } else {
                    vec![]
                };
                assert_eq!(
                    categories(&core_policy(&policy).check(&pair.pair.core, None)),
                    expected
                );
            }
        }
    }
    let mut bytes = vec![5, 4, 0];
    bytes.extend_from_slice(&EVM_MIRROR_KEY);
    bytes.push(1);
    let sweep = m5_pair(
        "transfer_all",
        IntentCall::transfer_all(EVM_MIRROR, true),
        FundEvmKey {
            evm_address: EVM_H160.into(),
            amount_tao: Amount::All,
        },
        bytes,
        vec![],
        Spend::Unbounded,
        false,
    );
    assert_m5_build(&sweep, &snapshot, &keys).await;
    for cap in CAPS {
        compare_m5_policy(
            &sweep,
            &Policy {
                max_spend: cap.map(Balance::<Tao>::from_rao),
                ..Default::default()
            },
            None,
        );
    }
    assert!(fake.mutate_state(|state| state.submissions.is_empty()));
    client.close().await;
}
