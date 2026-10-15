//! Compare actual M2/M4 intents with upstream trusted transaction constructors.
//!
//! Core diagnostics use integer rao, while rtao preserves Python's rendered
//! balances. Compare violation categories and order; assert Python wording
//! separately. Amounts outside native u64 are intentionally excluded (Q25).
#![allow(clippy::unwrap_used, clippy::panic)]

use rtao_client::{BlockRef, Client, ClientConfig, DynView, Network};
use rtao_intents::{
    BurnedRegister, RegisterSubnet, RootRegister, SetHyperparameter, StartCall, Transfer,
    TransferAll,
};
use rtao_primitives::{
    AccountId, Alpha, Amount, Balance, DynBalance, NetUid, Spend, Tao, UnitError,
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
use std::{collections::BTreeSet, sync::Arc};

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
