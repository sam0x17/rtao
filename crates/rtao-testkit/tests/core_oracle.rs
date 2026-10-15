//! Pinned-block differential checks against the upstream blocking client.
//!
//! All 21 non-retired trusted core constructors are paired with generated runtime
//! call builders. Policy checks use the implemented M2 Transfer/TransferAll
//! intents; the other 18 constructor counterparts require M5 implementations.

use std::{collections::BTreeSet, error::Error, time::Duration};

use bittensor_core::transaction::{IntentCall, Policy as CorePolicy};
use rtao_client::{BlockRef, View, ViewExt};
use rtao_core::{codec::Value, ops::fee};
use rtao_intents::{Transfer, TransferAll};
use rtao_primitives::{
    AccountId, AlphaBalance, Amount, Balance, NetUid, PerU16, PriceRao, Tao, TaoBalance,
};
use rtao_runtime::{
    Call,
    calls::{admin_utils, balances, subtensor_module as st_calls, system},
    storage::subtensor_module as st,
};
use rtao_signer::Signer;
use rtao_testkit::{Localnet, LocalnetProfile, Scenario};
use rtao_tx::{DynIntent, Policy};
use serde_json::json;

type TestResult = Result<(), Box<dyn Error + Send + Sync>>;

const M5_POLICY_GAPS: [&str; 18] = [
    "fund_evm_key",
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

struct ConstructorPair {
    name: &'static str,
    core: IntentCall,
    typed: Call,
}

// Core semantics come from the real constructor, including set_take's live
// Delegates query. The runtime side uses generated builders with independent
// positional inputs, so field order and argument encoding can disagree.
fn constructor_pairs(
    core: &bittensor_core::client::Client,
    hot: AccountId,
    destination: AccountId,
    netuid: NetUid,
    current_take: u16,
) -> Result<Vec<ConstructorPair>, Box<dyn Error + Send + Sync>> {
    let hot_ss58 = hot.to_string();
    let destination_ss58 = destination.to_string();
    let destination_hotkey = AccountId([43; 32]);
    let destination_hotkey_ss58 = destination_hotkey.to_string();
    let dest_value = || Value::Bytes(destination.0.to_vec()).into();
    let other = NetUid::ROOT;
    let tao = TaoBalance(1_000_000);
    let alpha = AlphaBalance(1_234_567);
    let price = PriceRao(987_654_321);
    let target_take = if current_take == 0 {
        1
    } else {
        current_take - 1
    };
    let take_call = if target_take < current_take {
        st_calls::decrease_take(hot, PerU16(target_take))
    } else {
        st_calls::increase_take(hot, PerU16(target_take))
    };
    let pairs = vec![
        ConstructorPair {
            name: "transfer",
            core: IntentCall::transfer(&destination_ss58, u128::from(tao.0)),
            typed: balances::transfer_keep_alive(dest_value(), tao),
        },
        ConstructorPair {
            name: "fund_evm_key",
            core: IntentCall::fund_evm_key(&destination_ss58, u128::from(tao.0)),
            typed: balances::transfer_keep_alive(dest_value(), tao),
        },
        ConstructorPair {
            name: "transfer_allow_death",
            core: IntentCall::transfer_allow_death(&destination_ss58, u128::from(tao.0)),
            typed: balances::transfer_allow_death(dest_value(), tao),
        },
        ConstructorPair {
            name: "transfer_all",
            core: IntentCall::transfer_all(&destination_ss58, false),
            typed: balances::transfer_all(dest_value(), false),
        },
        ConstructorPair {
            name: "add_stake",
            core: IntentCall::add_stake(&hot_ss58, netuid.0, u128::from(tao.0)),
            typed: st_calls::add_stake(hot, netuid, tao),
        },
        ConstructorPair {
            name: "add_stake_limit",
            core: IntentCall::add_stake_limit(
                &hot_ss58,
                netuid.0,
                u128::from(tao.0),
                u128::from(price.0),
                true,
            ),
            typed: st_calls::add_stake_limit(hot, netuid, tao, price, true),
        },
        ConstructorPair {
            name: "remove_stake",
            core: IntentCall::remove_stake(&hot_ss58, netuid.0, u128::from(alpha.0)),
            typed: st_calls::remove_stake(hot, netuid, alpha),
        },
        ConstructorPair {
            name: "remove_stake_limit",
            core: IntentCall::remove_stake_limit(
                &hot_ss58,
                netuid.0,
                u128::from(alpha.0),
                u128::from(price.0),
                false,
            ),
            typed: st_calls::remove_stake_limit(hot, netuid, alpha, price, false),
        },
        ConstructorPair {
            name: "register_subnet",
            core: IntentCall::register_subnet(&hot_ss58),
            typed: st_calls::register_network(hot),
        },
        ConstructorPair {
            name: "start_call",
            core: IntentCall::start_call(netuid.0),
            typed: st_calls::start_call(netuid),
        },
        ConstructorPair {
            name: "burned_register",
            core: IntentCall::burned_register(netuid.0, &hot_ss58),
            typed: st_calls::burned_register(netuid, hot),
        },
        ConstructorPair {
            name: "root_register",
            core: IntentCall::root_register(&hot_ss58),
            typed: st_calls::root_register(hot),
        },
        ConstructorPair {
            name: "move_stake",
            core: IntentCall::move_stake(
                &hot_ss58,
                netuid.0,
                &destination_ss58,
                other.0,
                u128::from(alpha.0),
            ),
            typed: st_calls::move_stake(hot, destination, netuid, other, alpha),
        },
        ConstructorPair {
            name: "move_stake_limit",
            core: IntentCall::move_stake_limit(
                &hot_ss58,
                netuid.0,
                &destination_ss58,
                other.0,
                u128::from(alpha.0),
                u128::from(price.0),
                true,
            ),
            typed: st_calls::move_stake_limit(hot, destination, netuid, other, alpha, price, true),
        },
        ConstructorPair {
            name: "swap_stake",
            core: IntentCall::swap_stake(&hot_ss58, netuid.0, other.0, u128::from(alpha.0)),
            typed: st_calls::swap_stake(hot, netuid, other, alpha),
        },
        ConstructorPair {
            name: "transfer_stake",
            core: IntentCall::transfer_stake(
                &destination_ss58,
                &hot_ss58,
                netuid.0,
                other.0,
                u128::from(alpha.0),
            ),
            typed: st_calls::transfer_stake(destination, hot, netuid, other, alpha),
        },
        ConstructorPair {
            name: "transfer_stake_and_hotkey",
            core: IntentCall::transfer_stake_and_hotkey(
                &destination_ss58,
                &hot_ss58,
                &destination_hotkey_ss58,
                netuid.0,
                other.0,
                u128::from(alpha.0),
            ),
            typed: st_calls::transfer_stake_and_hotkey(
                destination,
                hot,
                destination_hotkey,
                netuid,
                other,
                alpha,
            ),
        },
        ConstructorPair {
            name: "unstake_all",
            core: IntentCall::unstake_all(&hot_ss58),
            typed: st_calls::unstake_all(hot),
        },
        ConstructorPair {
            name: "unstake_all_alpha",
            core: IntentCall::unstake_all_alpha(&hot_ss58),
            typed: st_calls::unstake_all_alpha(hot),
        },
        ConstructorPair {
            name: "set_hyperparameter",
            core: IntentCall::set_hyperparameter(netuid.0, "min_allowed_weights", Value::Uint(7))?,
            typed: admin_utils::sudo_set_min_allowed_weights(netuid, 7),
        },
        ConstructorPair {
            name: "set_take",
            core: IntentCall::set_take(core, &hot_ss58, target_take)?,
            typed: take_call,
        },
    ];
    assert_eq!(pairs.len(), 21);
    assert_eq!(
        pairs
            .iter()
            .map(|pair| pair.name)
            .collect::<BTreeSet<_>>()
            .len(),
        21
    );
    Ok(pairs)
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Violation {
    RawCall,
    FeeUnavailable,
    FeeExceeded,
    SpendUnbounded,
    SpendExceeded,
    AllSubnets,
    Netuid(u16),
}

fn violation_set(messages: &[String]) -> Result<BTreeSet<Violation>, Box<dyn Error + Send + Sync>> {
    messages
        .iter()
        .map(|message| {
            Ok(if message.starts_with("raw call") {
                Violation::RawCall
            } else if message.contains("max_fee_") {
                if message.contains("could not be estimated")
                    || message.contains("estimation failed")
                {
                    Violation::FeeUnavailable
                } else if message.contains("exceeds") {
                    Violation::FeeExceeded
                } else {
                    return Err(format!("unrecognized fee diagnostic: {message}").into());
                }
            } else if message.contains("max_spend_") {
                if message.contains("unbounded") || message.contains("cannot be bounded") {
                    Violation::SpendUnbounded
                } else if message.contains("exceeds") {
                    Violation::SpendExceeded
                } else {
                    return Err(format!("unrecognized spend diagnostic: {message}").into());
                }
            } else if message.contains("all subnets") || message.contains("every subnet") {
                Violation::AllSubnets
            } else if message.starts_with("netuid ") {
                Violation::Netuid(
                    message
                        .split_whitespace()
                        .nth(1)
                        .ok_or("netuid diagnostic omitted value")?
                        .parse()?,
                )
            } else {
                return Err(format!("unrecognized policy diagnostic: {message}").into());
            })
        })
        .collect()
}

#[test]
fn core_oracle_m2_actual_intent_policy_violation_sets() -> TestResult {
    let destination = AccountId([42; 32]);
    let mut pairs: Vec<(&str, IntentCall, Box<dyn DynIntent>)> = Vec::new();
    for amount in [0, 1, 10, 11, 9_007_199_254_740_993, u64::MAX] {
        for keep_alive in [true, false] {
            let core = if keep_alive {
                IntentCall::transfer(destination.to_string(), u128::from(amount))
            } else {
                IntentCall::transfer_allow_death(destination.to_string(), u128::from(amount))
            };
            pairs.push((
                if keep_alive {
                    "transfer"
                } else {
                    "transfer_allow_death"
                },
                core,
                Box::new(Transfer {
                    dest_ss58: destination.into(),
                    amount_tao: Balance::<Tao>::from_rao(amount).into(),
                    keep_alive,
                }),
            ));
        }
    }
    for keep_alive in [true, false] {
        pairs.push((
            "transfer_all",
            IntentCall::transfer_all(destination.to_string(), keep_alive),
            Box::new(TransferAll {
                dest_ss58: destination.into(),
                keep_alive,
            }),
        ));
        pairs.push((
            "transfer_all",
            IntentCall::transfer_all(destination.to_string(), keep_alive),
            Box::new(Transfer {
                dest_ss58: destination.into(),
                amount_tao: Amount::All,
                keep_alive,
            }),
        ));
    }
    let caps = [
        None,
        Some(0),
        Some(10),
        Some(9_007_199_254_740_992),
        Some(u64::MAX),
    ];
    let fees = [None, Some(0), Some(10), Some(11), Some(u64::MAX)];
    let allowed = [
        None,
        Some(BTreeSet::new()),
        Some(BTreeSet::from([0_u16, 2])),
    ];
    let mut checks = 0;
    for (name, core, intent) in pairs {
        intent.validate_dyn()?;
        for max_fee in caps {
            for max_spend in caps {
                for allowlist in &allowed {
                    for allow_raw_calls in [false, true] {
                        let expected = CorePolicy {
                            max_fee_rao: max_fee.map(u128::from),
                            max_spend_rao: max_spend.map(u128::from),
                            allowed_netuids: allowlist.clone(),
                            allow_raw_calls,
                        };
                        let actual = Policy {
                            max_fee: max_fee.map(Balance::<Tao>::from_rao),
                            max_spend: max_spend.map(Balance::<Tao>::from_rao),
                            allowed_netuids: allowlist
                                .as_ref()
                                .map(|values| values.iter().copied().map(NetUid).collect()),
                            allow_raw_calls,
                        };
                        for fee in fees {
                            let core_messages = expected.check(&core, fee.map(u128::from));
                            let messages = actual.check(
                                intent.semantic_intent_dyn(),
                                fee.map(Balance::<Tao>::from_rao),
                            );
                            assert_eq!(
                                violation_set(&messages)?,
                                violation_set(&core_messages)?,
                                "{name}, policy={actual:?}, fee={fee:?}"
                            );
                            assert_eq!(messages.is_empty(), core_messages.is_empty());
                            checks += 1;
                        }
                    }
                }
            }
        }
    }
    assert_eq!(checks, 12_000);
    eprintln!(
        "M5 Policy coverage gap: {} real intent counterparts remain unavailable: {}",
        M5_POLICY_GAPS.len(),
        M5_POLICY_GAPS.join(", ")
    );
    Ok(())
}

#[tokio::test]
#[allow(clippy::disallowed_methods)] // Blocking upstream client is the differential oracle only.
async fn core_oracle_v0_pinned_storage_runtime_calls_and_identical_byte_fee() -> TestResult {
    let endpoint = std::env::var("E2E_ENDPOINT")?;
    let localnet = Localnet::connect(&endpoint, LocalnetProfile::Fast).await?;
    let scenario = Scenario::new(&localnet)
        .owner()
        .subnet()
        .start()
        .validator()
        .stake(TaoBalance(1_000_000_000_000))
        .build()
        .await?;
    let netuid = scenario.netuid.ok_or("oracle scenario omitted subnet")?;
    let setup_height = scenario
        .reports
        .iter()
        .filter_map(|report| report.block_number)
        .max()
        .ok_or("oracle setup has no inclusion height")?;
    let snapshot = tokio::time::timeout(Duration::from_secs(180), async {
        loop {
            let snapshot = localnet.client().at(BlockRef::Finalized).await?;
            if snapshot.block().number >= setup_height {
                return Ok::<_, rtao_client::ClientError>(snapshot);
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    })
    .await??;
    let hash = format!("0x{}", hex::encode(snapshot.block().hash.0));
    let runtime = snapshot.runtime().await?;
    let cold = scenario.slot.owner.account();
    let hot = scenario.slot.hotkey.account();
    let current_take = snapshot.storage(&st::DELEGATES, (hot,)).await?.0;
    let expected_spec = runtime.spec_version();
    let expected_tx = runtime.core().transaction_version;
    let constructor_runtime = runtime.clone();
    let storage_queries = [
        ("System", "Number", vec![]),
        ("Timestamp", "Now", vec![]),
        ("Drand", "LastStoredRound", vec![]),
        ("System", "Account", vec![Value::Bytes(cold.0.to_vec())]),
        (
            "SubtensorModule",
            "Tempo",
            vec![Value::Int(i128::from(netuid.get()))],
        ),
        (
            "SubtensorModule",
            "Owner",
            vec![Value::Bytes(hot.0.to_vec())],
        ),
    ];
    let api_queries = [
        (
            "SubnetInfoRuntimeApi",
            "get_metagraph",
            vec![Value::Int(i128::from(netuid.get()))],
        ),
        (
            "NeuronInfoRuntimeApi",
            "get_neurons_lite",
            vec![Value::Int(i128::from(netuid.get()))],
        ),
        (
            "SubnetInfoRuntimeApi",
            "get_subnet_hyperparams_v3",
            vec![Value::Int(i128::from(netuid.get()))],
        ),
        (
            "StakeInfoRuntimeApi",
            "get_stake_info_for_hotkey_coldkey_netuid",
            vec![
                Value::Bytes(hot.0.to_vec()),
                Value::Bytes(cold.0.to_vec()),
                Value::Int(i128::from(netuid.get())),
            ],
        ),
    ];
    let mut async_storage = Vec::new();
    for (pallet, item, keys) in &storage_queries {
        async_storage.push(snapshot.storage_dyn(pallet, item, keys).await?.into_core());
    }
    let mut async_api = Vec::new();
    for (api, method, args) in &api_queries {
        async_api.push(snapshot.call_dyn(api, method, args).await?.into_core());
    }
    let transfer =
        balances::transfer_keep_alive(Value::Bytes(hot.0.to_vec()).into(), TaoBalance(1_000_000))
            .compose(&runtime)?;
    let remark = system::remark(vec![1, 2, 3]).compose(&runtime)?;
    let nonce = localnet.client().nonces().fresh(&cold).await?;
    let probe = fee::probe(
        runtime.core(),
        transfer.as_ref(),
        cold.0,
        u8::from(scenario.slot.owner.crypto_type()),
        nonce,
        localnet.client().genesis().0,
    )?;
    let request = fee::request(&probe, Some(&hash))?;
    let response: serde_json::Value = localnet
        .client()
        .request(request.method, request.params)
        .await?;
    let async_fee = fee::decode(runtime.core(), &response)?.partial_fee;
    let extrinsic_hex = format!("0x{}", hex::encode(&probe));
    let expected_calls = [transfer.as_ref().to_vec(), remark.as_ref().to_vec()];
    let oracle = tokio::task::spawn_blocking(move || -> Result<_, Box<dyn Error + Send + Sync>> {
        let core = bittensor_core::client::Client::connect(endpoint)?;
        let core_runtime = core.runtime()?;
        assert_eq!(
            core_runtime.spec_version, expected_spec,
            "constructor oracle moved across a runtime upgrade"
        );
        assert_eq!(core_runtime.transaction_version, expected_tx);
        let mut core_storage = Vec::new();
        for (pallet, item, keys) in &storage_queries {
            core_storage.push(core.query(pallet, item, keys, Some(&hash))?);
        }
        let mut core_api = Vec::new();
        for (api, method, args) in &api_queries {
            core_api.push(core.runtime_call(api, method, args, Some(&hash))?);
        }
        let core_calls = [
            core.compose_call(
                "Balances",
                "transfer_keep_alive",
                &Value::record(vec![
                    ("dest".into(), Value::Bytes(hot.0.to_vec())),
                    ("value".into(), Value::Int(1_000_000)),
                ]),
            )?,
            core.compose_call(
                "System",
                "remark",
                &Value::record(vec![("remark".into(), Value::Bytes(vec![1, 2, 3]))]),
            )?,
        ];
        let fee = core.rpc_value("payment_queryInfo", json!([extrinsic_hex, hash]))?;
        let text = fee
            .get("partialFee")
            .and_then(serde_json::Value::as_str)
            .ok_or("core payment_queryInfo omitted partialFee text")?;
        let partial_fee: u64 = text.parse()?;
        let mut constructors = Vec::new();
        for pair in constructor_pairs(&core, hot, cold, netuid, current_take)? {
            constructors.push((
                pair.name,
                pair.typed.compose(&constructor_runtime)?.as_ref().to_vec(),
                pair.core.encode(&core)?,
            ));
        }
        Ok((
            core_storage,
            core_api,
            core_calls,
            partial_fee,
            constructors,
        ))
    });
    let (core_storage, core_api, core_calls, core_fee, constructors) =
        tokio::time::timeout(Duration::from_secs(180), oracle).await???;
    assert_eq!(async_storage, core_storage, "pinned storage values differ");
    assert_eq!(async_api, core_api, "pinned runtime-API values differ");
    assert_eq!(expected_calls, core_calls, "typed setup calls differ");
    assert_eq!(constructors.len(), 21);
    for (name, actual, expected) in constructors {
        assert_eq!(
            actual, expected,
            "trusted constructor {name} call bytes differ"
        );
    }
    assert_eq!(
        async_fee, core_fee,
        "fee differs on identical complete extrinsic bytes"
    );
    localnet.client().close().await;
    Ok(())
}
