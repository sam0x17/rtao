//! Exercise recursive policy and mandatory shielding through real tool dispatch.
//! FakeChain verifies review, policy and signing; it does not execute the
//! callback or prove Crowdloan's runtime FundsNotSettled settlement condition.
#![cfg(feature = "all")]
#![allow(clippy::expect_used)]

use anyhow::Result;
use rtao_client::{Client, ClientConfig, Network};
use rtao_intents::{Batch, ExecuteProxyAnnounced, MultisigThreshold1};
use rtao_primitives::{AccountId, Balance, CryptoType, NetUid, Spend, Tao, TaoBalance};
use rtao_runtime::{
    CallBytes, calls,
    models::{CrowdloanCall, CrowdloanInfo},
    storage::{crowdloan::CROWDLOANS, subtensor_module as st},
};
use rtao_signer::{
    BoxFuture, DynKeySource, KeyView, KeypairSigner, MultiSignature, SignPurpose, Signer,
    SignerError, SignerRole, SigningRequest,
};
use rtao_testkit::{FakeChain, FakeState, rpc::RpcClient};
use rtao_tx::{
    BuildCx, ExecuteOptions, Executor, Intent, OriginView, PlanOptions, Policy, ShieldOptions,
    SystemClock, TxError,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

const OTHER: AccountId = AccountId([8; 32]);
const DEST: AccountId = AccountId([9; 32]);

#[derive(Clone)]
struct RecordingSigner {
    key: KeypairSigner,
    signed: Arc<Mutex<Vec<(SignPurpose, Vec<u8>)>>>,
}

impl Signer for RecordingSigner {
    fn account(&self) -> AccountId {
        self.key.account()
    }
    fn crypto_type(&self) -> CryptoType {
        self.key.crypto_type()
    }
    fn sign<'a>(
        &'a self,
        request: &'a SigningRequest<'a>,
    ) -> BoxFuture<'a, Result<MultiSignature, SignerError>> {
        Box::pin(async move {
            self.signed
                .lock()
                .expect("signed calls")
                .push((request.purpose, request.call.to_vec()));
            self.key.sign(request).await
        })
    }
}

struct Keys {
    signer: RecordingSigner,
    acquisitions: AtomicUsize,
}

impl KeyView for Keys {
    fn hotkey_address(&self) -> Result<AccountId, SignerError> {
        Ok(self.signer.account())
    }
    fn hotkey_public_key(&self) -> Result<[u8; 32], SignerError> {
        Ok(self.signer.account().0)
    }
    fn coldkey_address(&self) -> Result<AccountId, SignerError> {
        Ok(self.signer.account())
    }
}

impl DynKeySource for Keys {
    fn signer_for<'a>(
        &'a self,
        role: SignerRole,
    ) -> BoxFuture<'a, Result<Arc<dyn Signer>, SignerError>> {
        Box::pin(async move {
            assert_eq!(role, SignerRole::Coldkey);
            self.acquisitions.fetch_add(1, Ordering::SeqCst);
            Ok(Arc::new(self.signer.clone()) as Arc<dyn Signer>)
        })
    }
}

async fn fixture() -> Result<(Executor, FakeChain, Keys)> {
    let keys = Keys {
        signer: RecordingSigner {
            key: KeypairSigner::from_seed(&[7; 32], CryptoType::Ed25519)?,
            signed: Arc::new(Mutex::new(vec![])),
        },
        acquisitions: AtomicUsize::new(0),
    };
    let mut state = FakeState::try_default()?;
    state.fee = 7;
    state.seed_constant("Multisig", "DepositBase", json!(100))?;
    state.seed_constant("Multisig", "DepositFactor", json!(10))?;
    state.seed_python(
        "System",
        "Account",
        &[json!(keys.signer.account())],
        json!({"data":{"free":1_000_000_000_000u64}}),
    )?;
    state.seed_typed("MevShield", "NextKey", (), vec![0u8; 1184])?;
    state.seed_constant("Aura", "SlotDuration", json!(1))?;
    state.seed(&st::BURN, (NetUid(3),), TaoBalance(10))?;
    state.seed(&st::COLLATERAL_LOCK_SHARE, (NetUid(3),), 0u16)?;
    state.seed_runtime("SwapRuntimeApi", "current_alpha_price", json!(20))?;
    let chain = FakeChain::from_state(state);
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    let client = Client::from_rpc(RpcClient::new(chain.clone()), config).await?;
    Ok((rtao_intents::executor(client), chain, keys))
}

fn pool() -> ExecuteOptions {
    ExecuteOptions {
        wait_for_inclusion: false,
        wait_for_finalization: false,
        ..Default::default()
    }
}

fn threshold_one(child: Value) -> Value {
    json!({"op":"multisig_threshold_1","other_signatories":[OTHER],"call":child})
}

fn proxy(child: Value) -> Value {
    json!({"op":"execute_proxy_announced","delegate_ss58":OTHER,"real_ss58":DEST,
        "inner_op":child["op"],"inner_args":child})
}

fn wrappers(child: Value) -> Vec<Value> {
    vec![
        threshold_one(child.clone()),
        json!({"op":"multisig_execute","threshold":2,"other_signatories":[OTHER],"call":child}),
        json!({"op":"multisig_approve","threshold":2,"other_signatories":[OTHER],"call":child}),
        proxy(child.clone()),
        json!({"op":"batch","intents":[child]}),
        json!({"op":"batch","intents":[threshold_one(proxy(child))]}),
    ]
}

fn no_private_or_submit(chain: &FakeChain, keys: &Keys) {
    assert_eq!(keys.acquisitions.load(Ordering::SeqCst), 0);
    assert!(keys.signer.signed.lock().expect("signed calls").is_empty());
    assert!(chain.mutate_state(|state| state.submissions.is_empty()));
}

fn seed_preimage(chain: &FakeChain, call: &CallBytes, hash: [u8; 32], len: u32) -> Result<()> {
    chain.mutate_state(|state| {
        let hash = json!(format!("0x{}", hex::encode(hash)));
        let keys = match state
            .runtime
            .storage_entry("Preimage", "PreimageFor")
            .expect("preimage storage")
            .key_types
            .len()
        {
            1 => vec![json!([hash, len])],
            2 => vec![hash, json!(len)],
            count => anyhow::bail!("unsupported test preimage key count {count}"),
        };
        state.seed_python(
            "Preimage",
            "PreimageFor",
            &keys,
            json!(format!("0x{}", hex::encode(call.as_bytes()))),
        )?;
        Ok(())
    })
}

fn seed_callback(chain: &FakeChain, keys: &Keys, id: u32, call: &CallBytes) -> Result<()> {
    let bounded = if call.as_bytes().len() <= 128 {
        CrowdloanCall::Inline(call.as_bytes().to_vec())
    } else {
        let len = u32::try_from(call.as_bytes().len())?;
        seed_preimage(chain, call, call.hash(), len)?;
        CrowdloanCall::Lookup {
            hash: call.hash(),
            len,
        }
    };
    seed_loan(chain, keys, id, Some(bounded))
}

fn seed_loan(chain: &FakeChain, keys: &Keys, id: u32, call: Option<CrowdloanCall>) -> Result<()> {
    let target_address = call.is_none().then_some(DEST);
    chain.mutate_state(|state| {
        state.seed(
            &CROWDLOANS,
            (id,),
            CrowdloanInfo {
                creator: keys.signer.account(),
                deposit: TaoBalance(1),
                min_contribution: TaoBalance(1),
                end: 100,
                cap: TaoBalance(10),
                funds_account: OTHER,
                raised: TaoBalance(10),
                target_address,
                call,
                finalized: false,
                contributors_count: 1,
            },
        )
    })?;
    Ok(())
}

async fn child_bytes(executor: &Executor, keys: &Keys, child: Value) -> Result<CallBytes> {
    let intent = rtao_intents::AnyIntent::from_tool(
        child["op"].as_str().expect("operation"),
        child.clone(),
    )?;
    let snapshot = executor
        .client()
        .at(rtao_client::BlockRef::Finalized)
        .await?;
    Ok(intent
        .as_dyn()
        .build_dyn(&BuildCx {
            view: &snapshot,
            origin: OriginView::Keys(keys),
            runtime: &snapshot.block().runtime,
            clock: &SystemClock,
            spec: intent.spec(),
        })
        .await?
        .call)
}

fn create_with(child: Value) -> Value {
    json!({"op":"create_crowdloan","deposit_tao":"0.000000001","min_contribution_tao":"0",
        "cap_tao":"1","end":100,"call":child})
}

#[tokio::test]
async fn deferred_creation_keeps_child_spend_subnet_and_shield_requirements() -> Result<()> {
    for (child, policy, diagnostic) in [
        (
            json!({"op":"transfer_all","dest_ss58":DEST}),
            Policy {
                max_spend: Some(Balance::ZERO),
                ..Default::default()
            },
            "max_spend_tao",
        ),
        (
            json!({"op":"transfer","dest_ss58":DEST,"amount_tao":"0.000000002"}),
            Policy {
                max_spend: Some(Balance::from_rao(2)),
                ..Default::default()
            },
            "max_spend_tao",
        ),
        (
            json!({"op":"set_subnet_emission_enabled","netuids":[3],"enabled":true}),
            Policy {
                allowed_netuids: Some(BTreeSet::from([NetUid(2)])),
                ..Default::default()
            },
            "netuid 3",
        ),
    ] {
        for args in wrappers(create_with(child)) {
            let (executor, chain, keys) = fixture().await?;
            let error = executor
                .execute_tool(
                    args["op"].as_str().expect("operation"),
                    args.clone(),
                    &keys,
                    &ExecuteOptions {
                        policy: Some(policy.clone()),
                        ..pool()
                    },
                )
                .await
                .expect_err("nested authorization must respect child policy");
            assert!(error.to_string().contains(diagnostic), "{args}: {error}");
            no_private_or_submit(&chain, &keys);
            executor.client().close().await;
        }
    }
    for child in [
        json!({"op":"burned_register","netuid":3,"hotkey_ss58":DEST}),
        json!({"op":"add_collateral","netuid":3,"hotkey_ss58":DEST,"amount_alpha":"1"}),
    ] {
        for args in wrappers(create_with(threshold_one(proxy(child)))) {
            let (executor, chain, keys) = fixture().await?;
            let before = chain.requests().len();
            let error = executor
                .execute_tool(
                    args["op"].as_str().expect("operation"),
                    args.clone(),
                    &keys,
                    &pool(),
                )
                .await
                .expect_err("deferred shielding is unsupported");
            assert!(
                error.to_string().contains("deferred child dispatch"),
                "{error}"
            );
            assert_eq!(chain.requests().len(), before);
            no_private_or_submit(&chain, &keys);
            executor.client().close().await;
        }
    }
    Ok(())
}

#[tokio::test]
async fn stored_callbacks_propagate_spend_subnets_and_global_policy_through_tools() -> Result<()> {
    for (child, policy, diagnostic) in [
        (
            json!({"op":"transfer_all","dest_ss58":DEST}),
            Policy {
                max_spend: Some(Balance::ZERO),
                ..Default::default()
            },
            "max_spend_tao",
        ),
        (
            json!({"op":"transfer","dest_ss58":DEST,"amount_tao":"0.000000002"}),
            Policy {
                max_spend: Some(Balance::from_rao(1)),
                ..Default::default()
            },
            "max_spend_tao",
        ),
        (
            json!({"op":"set_subnet_emission_enabled","netuids":[3],"enabled":true}),
            Policy {
                allowed_netuids: Some(BTreeSet::from([NetUid(2)])),
                ..Default::default()
            },
            "netuid 3",
        ),
        (
            json!({"op":"unstake_all_alpha","hotkey_ss58":DEST}),
            Policy {
                allowed_netuids: Some(BTreeSet::from([NetUid(2)])),
                ..Default::default()
            },
            "acts on all subnets",
        ),
    ] {
        for args in wrappers(json!({"op":"finalize_crowdloan","crowdloan_id":42})) {
            let (executor, chain, keys) = fixture().await?;
            let callback =
                child_bytes(&executor, &keys, threshold_one(proxy(child.clone()))).await?;
            seed_callback(&chain, &keys, 42, &callback)?;
            let error = executor
                .execute_tool(
                    args["op"].as_str().expect("operation"),
                    args.clone(),
                    &keys,
                    &ExecuteOptions {
                        policy: Some(policy.clone()),
                        ..pool()
                    },
                )
                .await
                .expect_err("stored child policy must survive wrappers");
            assert!(error.to_string().contains(diagnostic), "{args}: {error}");
            no_private_or_submit(&chain, &keys);
            executor.client().close().await;
        }
    }
    Ok(())
}

#[tokio::test]
async fn recognized_stored_callbacks_and_target_loans_preserve_finalization_bytes() -> Result<()> {
    for target in [false, true] {
        let (executor, chain, keys) = fixture().await?;
        let callback = child_bytes(
            &executor,
            &keys,
            json!({"op":"batch","intents":[
                {"op":"transfer","dest_ss58":DEST,"amount_tao":"0.000000002"},
                {"op":"set_subnet_emission_enabled","netuids":[3],"enabled":true}
            ]}),
        )
        .await?;
        if target {
            seed_loan(&chain, &keys, 42, None)?;
        } else {
            seed_callback(&chain, &keys, 42, &callback)?;
        }
        let intent = rtao_intents::FinalizeCrowdloan { crowdloan_id: 42 };
        let plan = executor
            .plan_dyn(
                &intent,
                &keys,
                &PlanOptions {
                    policy: (!target).then(|| Policy {
                        max_spend: Some(Balance::from_rao(2)),
                        allowed_netuids: Some(BTreeSet::from([NetUid(3)])),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            )
            .await?;
        assert!(plan.ok(), "{:?}", plan.violations);
        assert_eq!(
            plan.spend,
            if target {
                Spend::Unbounded
            } else {
                Spend::Bounded(Balance::from_rao(2))
            }
        );
        let runtime = executor.client().refresh_runtime().await?;
        assert_eq!(
            plan.call(),
            &calls::crowdloan::finalize(42).compose(&runtime)?
        );
        let result = executor
            .execute_tool(
                "finalize_crowdloan",
                json!({"crowdloan_id":42}),
                &keys,
                &ExecuteOptions {
                    expected_call: Some(plan.call().clone()),
                    ..pool()
                },
            )
            .await?;
        assert!(result.success);
        assert_eq!(chain.mutate_state(|state| state.submissions.len()), 1);
        executor.client().close().await;
    }
    Ok(())
}

#[tokio::test]
async fn plural_subnet_intent_callbacks_refuse_any_forbidden_uid_after_wire_batching() -> Result<()>
{
    let (executor, chain, keys) = fixture().await?;
    let callback = child_bytes(
        &executor,
        &keys,
        json!({"op":"set_subnet_emission_enabled","netuids":[3,4],"enabled":true}),
    )
    .await?;
    seed_callback(&chain, &keys, 42, &callback)?;
    let error = executor
        .execute_tool(
            "finalize_crowdloan",
            json!({"crowdloan_id":42}),
            &keys,
            &ExecuteOptions {
                policy: Some(Policy {
                    allowed_netuids: Some(BTreeSet::from([NetUid(3)])),
                    ..Default::default()
                }),
                ..pool()
            },
        )
        .await
        .expect_err("every scalar call in plural source intent must retain its scope");
    assert!(error.to_string().contains("netuid 4 not in"), "{error}");
    no_private_or_submit(&chain, &keys);
    executor.client().close().await;
    Ok(())
}

#[tokio::test]
async fn stored_wire_safety_matches_registered_semantic_amount_scope_and_alias_hooks() -> Result<()>
{
    let (executor, chain, keys) = fixture().await?;
    for child in [
        json!({"op":"transfer","dest_ss58":DEST,"amount_tao":"0.000000002","keep_alive":false}),
        json!({"op":"transfer_all","dest_ss58":DEST}),
        json!({"op":"fund_evm_key","evm_address":"0x0101010101010101010101010101010101010101","amount_tao":"0.000000002"}),
        json!({"op":"add_stake","hotkey_ss58":DEST,"netuid":3,"amount_tao":"0.000000002","slippage_protection":false}),
        json!({"op":"add_stake_limit","hotkey_ss58":DEST,"netuid":3,"amount_tao":"0.000000002","limit_price_rao":100}),
        json!({"op":"stake_into_basket","hotkey_ss58":DEST,"amount_tao":"0.000000002"}),
        json!({"op":"stake_burn","hotkey_ss58":DEST,"netuid":3,"amount_tao":"0.000000002","limit_price":100}),
        json!({"op":"register_subnet","hotkey_ss58":DEST}),
        json!({"op":"register_leased_network","emissions_share":50}),
        json!({"op":"contribute_crowdloan","crowdloan_id":43,"amount_tao":"0.000000002"}),
        json!({"op":"transfer_stake","dest_coldkey_ss58":DEST,"hotkey_ss58":DEST,"origin_netuid":3,"dest_netuid":4,"amount_alpha":"0.000000002"}),
        json!({"op":"swap_stake","hotkey_ss58":DEST,"origin_netuid":3,"dest_netuid":4,"amount_alpha":"0.000000002","slippage_protection":false}),
        json!({"op":"move_stake","origin_hotkey_ss58":OTHER,"dest_hotkey_ss58":DEST,"origin_netuid":3,"dest_netuid":4,"amount_alpha":"0.000000002","slippage_protection":false}),
        json!({"op":"swap_basket","hotkey_ss58":DEST,"origin_netuid":3,"dest_netuid":4,"amount":"0.000000002","min_amount_out":"0"}),
        json!({"op":"swap_basket_many","hotkey_ss58":DEST,"legs":[{"origin_netuid":3,"dest_netuid":4,"amount":"0.000000002"},{"origin_netuid":4,"dest_netuid":5,"amount":"0.000000002"}]}),
        json!({"op":"swap_coldkey_announced","new_coldkey_ss58":DEST}),
        json!({"op":"unstake_all","hotkey_ss58":DEST}),
        json!({"op":"unstake_all_alpha","hotkey_ss58":DEST}),
        json!({"op":"root_register","hotkey_ss58":DEST}),
        json!({"op":"set_root_claim_threshold","threshold":"0.000000002"}),
        json!({"op":"set_subnet_emission_enabled","netuids":[3,4],"enabled":true}),
        json!({"op":"swap_hotkey","hotkey_ss58":OTHER,"new_hotkey_ss58":DEST,"netuid":3}),
        json!({"op":"swap_hotkey","hotkey_ss58":OTHER,"new_hotkey_ss58":DEST}),
    ] {
        let intent = rtao_intents::AnyIntent::from_tool(
            child["op"].as_str().expect("operation"),
            child.clone(),
        )?;
        let semantic = intent.as_dyn().safety_dyn()?;
        let callback = child_bytes(&executor, &keys, child.clone()).await?;
        seed_callback(&chain, &keys, 42, &callback)?;
        let plan = executor
            .plan_dyn(
                &rtao_intents::FinalizeCrowdloan { crowdloan_id: 42 },
                &keys,
                &PlanOptions {
                    policy: Some(Policy {
                        allowed_netuids: Some(BTreeSet::new()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            )
            .await?;
        assert_eq!(plan.spend, semantic.spend, "{child}");
        let expected = Policy {
            allowed_netuids: Some(BTreeSet::new()),
            ..Default::default()
        }
        .check(&semantic, plan.fee);
        assert_eq!(
            plan.violations.len(),
            expected.len(),
            "{child}: {:?} != {expected:?}",
            plan.violations
        );
        for uid in semantic.netuids {
            assert!(
                plan.violations
                    .iter()
                    .any(|message| message.starts_with(&format!("netuid {} ", uid.0))),
                "{child}: {:?}",
                plan.violations
            );
        }
    }
    no_private_or_submit(&chain, &keys);
    executor.client().close().await;
    Ok(())
}

#[tokio::test]
async fn deferred_required_shield_callbacks_refuse_even_explicit_shield_submission() -> Result<()> {
    for child in [
        json!({"op":"burned_register","netuid":3,"hotkey_ss58":DEST}),
        json!({"op":"add_collateral","netuid":3,"hotkey_ss58":DEST,"amount_alpha":"1"}),
    ] {
        let (executor, chain, keys) = fixture().await?;
        let callback = child_bytes(&executor, &keys, threshold_one(proxy(child))).await?;
        seed_callback(&chain, &keys, 42, &callback)?;
        let intent = rtao_intents::FinalizeCrowdloan { crowdloan_id: 42 };
        for shielded in [false, true] {
            let result = if shielded {
                executor
                    .submit_shielded_dyn(
                        &intent,
                        &keys,
                        &ShieldOptions {
                            execute: pool(),
                            ..Default::default()
                        },
                    )
                    .await
            } else {
                executor.execute_dyn(&intent, &keys, &pool()).await
            };
            assert!(
                result
                    .expect_err("deferred shield must refuse")
                    .to_string()
                    .contains("deferred child dispatch")
            );
            no_private_or_submit(&chain, &keys);
        }
        executor.client().close().await;
    }
    Ok(())
}

#[tokio::test]
async fn stored_lookup_and_legacy_preimages_authenticate_length_hash_and_policy() -> Result<()> {
    for legacy in [false, true] {
        let (executor, chain, keys) = fixture().await?;
        let callback = child_bytes(
            &executor,
            &keys,
            json!({"op":"transfer","dest_ss58":DEST,"amount_tao":"0.000000002"}),
        )
        .await?;
        let len = u32::try_from(callback.as_bytes().len())?;
        seed_preimage(&chain, &callback, callback.hash(), len)?;
        if legacy {
            chain.mutate_state(|state| {
                let name = ["RequestStatusFor", "StatusFor"]
                    .into_iter()
                    .find(|name| state.runtime.storage_entry("Preimage", name).is_some())
                    .expect("preimage status");
                state.seed_python(
                    "Preimage",
                    name,
                    &[json!(format!("0x{}", hex::encode(callback.hash())))],
                    json!({"Requested":{"count":1,"len":len}}),
                )
            })?;
        }
        seed_loan(
            &chain,
            &keys,
            42,
            Some(if legacy {
                CrowdloanCall::Legacy {
                    hash: callback.hash(),
                }
            } else {
                CrowdloanCall::Lookup {
                    hash: callback.hash(),
                    len,
                }
            }),
        )?;
        let intent = rtao_intents::FinalizeCrowdloan { crowdloan_id: 42 };
        let plan = executor
            .plan_dyn(
                &intent,
                &keys,
                &PlanOptions {
                    policy: Some(Policy {
                        max_spend: Some(Balance::from_rao(2)),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            )
            .await?;
        assert!(plan.ok(), "{:?}", plan.violations);
        assert_eq!(plan.spend, Spend::Bounded(Balance::from_rao(2)));
        for (hash, recorded_len, seeded_len, diagnostic) in [
            ([12; 32], len, len, "hash does not match"),
            (callback.hash(), len + 1, len + 1, "length does not match"),
            ([13; 32], len, len + 1, "preimage is missing"),
        ] {
            seed_preimage(&chain, &callback, hash, seeded_len)?;
            seed_loan(
                &chain,
                &keys,
                42,
                Some(CrowdloanCall::Lookup {
                    hash,
                    len: recorded_len,
                }),
            )?;
            let error = executor
                .execute_tool(
                    "finalize_crowdloan",
                    json!({"crowdloan_id":42}),
                    &keys,
                    &pool(),
                )
                .await
                .expect_err("preimage integrity must refuse");
            assert!(error.to_string().contains(diagnostic), "{error}");
            no_private_or_submit(&chain, &keys);
        }
        executor.client().close().await;
    }
    Ok(())
}

#[tokio::test]
async fn opaque_missing_cyclic_and_over_budget_callbacks_fail_closed_before_signing() -> Result<()>
{
    let (executor, chain, keys) = fixture().await?;
    let runtime = executor.client().refresh_runtime().await?;
    let unsupported = calls::system::set_code(vec![1, 2]).compose(&runtime)?;
    let hash_only = child_bytes(
        &executor,
        &keys,
        json!({"op":"multisig_approve","threshold":2,
        "other_signatories":[OTHER],"call":{"op":"transfer_all","dest_ss58":DEST},
        "timepoint":{"height":1,"index":0}}),
    )
    .await?;
    for callback in [unsupported, hash_only] {
        seed_callback(&chain, &keys, 42, &callback)?;
        for allow_raw_calls in [false, true] {
            let error = executor
                .execute_tool(
                    "finalize_crowdloan",
                    json!({"crowdloan_id":42}),
                    &keys,
                    &ExecuteOptions {
                        policy: Some(Policy {
                            allow_raw_calls,
                            ..Default::default()
                        }),
                        ..pool()
                    },
                )
                .await
                .expect_err("unknown callback cannot claim verified policy or transport");
            assert!(
                error
                    .to_string()
                    .contains("cannot verify deferred crowdloan callback"),
                "{error}"
            );
            no_private_or_submit(&chain, &keys);
        }
    }
    seed_callback(
        &chain,
        &keys,
        42,
        &calls::crowdloan::finalize(43).compose(&runtime)?,
    )?;
    seed_callback(
        &chain,
        &keys,
        43,
        &calls::crowdloan::finalize(42).compose(&runtime)?,
    )?;
    let error = executor
        .execute_tool(
            "finalize_crowdloan",
            json!({"crowdloan_id":42}),
            &keys,
            &pool(),
        )
        .await
        .expect_err("callback cycle");
    assert!(error.to_string().contains("cyclic or repeated"), "{error}");
    seed_loan(&chain, &keys, 43, None)?;
    let nested = calls::crowdloan::create(
        TaoBalance(0),
        TaoBalance(0),
        TaoBalance(0),
        100,
        Some(calls::crowdloan::finalize(43).compose(&runtime)?),
        None,
    )
    .compose(&runtime)?;
    seed_callback(&chain, &keys, 42, &nested)?;
    let error = executor
        .execute_tool(
            "finalize_crowdloan",
            json!({"crowdloan_id":42}),
            &keys,
            &pool(),
        )
        .await
        .expect_err("nested future stored source");
    assert!(
        error
            .to_string()
            .contains("source may change before dispatch"),
        "{error}"
    );
    let error = executor
        .execute_tool(
            "create_crowdloan",
            create_with(threshold_one(proxy(
                json!({"op":"finalize_crowdloan","crowdloan_id":43}),
            ))),
            &keys,
            &pool(),
        )
        .await
        .expect_err("creation must not freeze mutable future source inspection");
    assert!(
        error
            .to_string()
            .contains("source may change before dispatch"),
        "{error}"
    );
    let mut deep = calls::system::remark(vec![]).compose(&runtime)?;
    for _ in 0..33 {
        deep = calls::utility::batch_all(vec![deep]).compose(&runtime)?;
    }
    seed_callback(&chain, &keys, 42, &deep)?;
    let error = executor
        .execute_tool(
            "finalize_crowdloan",
            json!({"crowdloan_id":42}),
            &keys,
            &pool(),
        )
        .await
        .expect_err("callback depth budget");
    assert!(error.to_string().contains("depth budget"), "{error}");
    let wide =
        calls::utility::batch_all(vec![calls::system::remark(vec![]).compose(&runtime)?; 4097])
            .compose(&runtime)?;
    seed_callback(&chain, &keys, 42, &wide)?;
    let error = executor
        .execute_tool(
            "finalize_crowdloan",
            json!({"crowdloan_id":42}),
            &keys,
            &pool(),
        )
        .await
        .expect_err("callback node budget");
    assert!(error.to_string().contains("node budget"), "{error}");
    seed_loan(
        &chain,
        &keys,
        42,
        Some(CrowdloanCall::Lookup {
            hash: [14; 32],
            len: 4 * 1024 * 1024 + 1,
        }),
    )?;
    let error = executor
        .execute_tool(
            "finalize_crowdloan",
            json!({"crowdloan_id":42}),
            &keys,
            &pool(),
        )
        .await
        .expect_err("callback byte budget");
    assert!(error.to_string().contains("byte budget"), "{error}");
    let error = executor
        .execute_tool(
            "finalize_crowdloan",
            json!({"crowdloan_id":9999}),
            &keys,
            &pool(),
        )
        .await
        .expect_err("missing callback state");
    assert!(
        error.to_string().contains("crowdloan 9999 is unavailable"),
        "{error}"
    );
    no_private_or_submit(&chain, &keys);
    executor.client().close().await;
    Ok(())
}

#[tokio::test]
async fn nested_tools_cannot_hide_unbounded_or_bounded_child_spend() -> Result<()> {
    for (child, cap) in [
        (json!({"op":"transfer_all","dest_ss58":DEST}), 0),
        (
            json!({"op":"transfer","dest_ss58":DEST,"amount_tao":"0.000000002"}),
            1,
        ),
    ] {
        for args in wrappers(child) {
            let direct = rtao_intents::AnyIntent::from_tool(
                args["op"].as_str().expect("operation"),
                args.clone(),
            )?;
            let violations = Policy {
                max_spend: Some(Balance::from_rao(cap)),
                allow_raw_calls: false,
                ..Default::default()
            }
            .check(direct.as_dyn(), None);
            assert!(
                violations
                    .iter()
                    .any(|message| message.contains("max_spend_tao")),
                "direct policy check lost nested spend: {args}: {violations:?}"
            );
            let (executor, chain, keys) = fixture().await?;
            let result = executor
                .execute_tool(
                    args["op"].as_str().expect("operation"),
                    args.clone(),
                    &keys,
                    &ExecuteOptions {
                        policy: Some(Policy {
                            max_spend: Some(Balance::from_rao(cap)),
                            allow_raw_calls: false,
                            ..Default::default()
                        }),
                        ..pool()
                    },
                )
                .await;
            assert!(
                matches!(&result, Err(TxError::Policy(_))),
                "{args}: {result:?}"
            );
            assert!(
                result
                    .expect_err("spend cap")
                    .to_string()
                    .contains("max_spend_tao")
            );
            no_private_or_submit(&chain, &keys);
            executor.client().close().await;
        }
    }
    Ok(())
}

#[tokio::test]
async fn nested_tools_preserve_child_subnet_and_global_restrictions() -> Result<()> {
    for (child, diagnostic) in [
        (
            json!({"op":"set_subnet_emission_enabled","netuids":[3],"enabled":true}),
            "netuid 3 not in",
        ),
        (
            json!({"op":"unstake_all_alpha","hotkey_ss58":DEST}),
            "acts on all subnets",
        ),
    ] {
        for args in wrappers(child) {
            let direct = rtao_intents::AnyIntent::from_tool(
                args["op"].as_str().expect("operation"),
                args.clone(),
            )?;
            let violations = Policy {
                allowed_netuids: Some(BTreeSet::from([NetUid(2)])),
                ..Default::default()
            }
            .check(direct.as_dyn(), None);
            assert!(
                violations
                    .iter()
                    .any(|message| message.contains(diagnostic)),
                "direct policy check lost nested subnet scope: {args}: {violations:?}"
            );
            let (executor, chain, keys) = fixture().await?;
            let result = executor
                .execute_tool(
                    args["op"].as_str().expect("operation"),
                    args.clone(),
                    &keys,
                    &ExecuteOptions {
                        policy: Some(Policy {
                            allowed_netuids: Some(BTreeSet::from([NetUid(2)])),
                            ..Default::default()
                        }),
                        ..pool()
                    },
                )
                .await;
            assert!(
                matches!(&result, Err(TxError::Policy(_))),
                "{args}: {result:?}"
            );
            assert!(
                result
                    .expect_err("subnet restriction")
                    .to_string()
                    .contains(diagnostic)
            );
            no_private_or_submit(&chain, &keys);
            executor.client().close().await;
        }
    }
    Ok(())
}

#[tokio::test]
async fn batch_sums_spend_through_distinct_wrappers_and_rejects_native_overflow() -> Result<()> {
    for (amount, expected) in [
        ("0.000000001", Spend::Bounded(Balance::<Tao>::from_rao(2))),
        ("18446744073.709551615", Spend::Unbounded),
    ] {
        let child = json!({"op":"transfer","dest_ss58":DEST,"amount_tao":amount});
        let batch = Batch::new(vec![threshold_one(child.clone()), proxy(child)])?;
        let (executor, chain, keys) = fixture().await?;
        let plan = executor
            .plan_dyn(
                &batch,
                &keys,
                &PlanOptions {
                    policy: Some(Policy {
                        max_spend: Some(Balance::from_rao(1)),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            )
            .await?;
        assert_eq!(plan.spend, expected);
        assert!(!plan.ok());
        no_private_or_submit(&chain, &keys);
        executor.client().close().await;
    }
    Ok(())
}

#[tokio::test]
async fn mandatory_child_shielding_survives_tools_and_preserves_reviewed_inner_bytes() -> Result<()>
{
    for args in [
        json!({"op":"burned_register","netuid":3,"hotkey_ss58":DEST}),
        json!({"op":"add_collateral","netuid":3,"hotkey_ss58":DEST,"amount_alpha":"0.000000001"}),
    ]
    .into_iter()
    .flat_map(wrappers)
    {
        let (executor, chain, keys) = fixture().await?;
        let intent = rtao_intents::AnyIntent::from_tool(
            args["op"].as_str().expect("operation"),
            args.clone(),
        )?;
        assert!(intent.as_dyn().safety_dyn()?.mev_shield_required);
        let plan = executor
            .plan_dyn(intent.as_dyn(), &keys, &PlanOptions::default())
            .await?;
        let result = executor
            .execute_tool(
                args["op"].as_str().expect("operation"),
                args.clone(),
                &keys,
                &ExecuteOptions {
                    expected_call: Some(plan.call().clone()),
                    ..pool()
                },
            )
            .await?;
        assert!(result.success);
        {
            let signed = keys.signer.signed.lock().expect("signed calls");
            assert_eq!(signed.len(), 2);
            assert_eq!(
                signed[0],
                (SignPurpose::ShieldInner, plan.call().as_bytes().to_vec())
            );
            assert_eq!(signed[1].0, SignPurpose::ShieldCarrier);
        }
        assert_eq!(chain.mutate_state(|state| state.submissions.len()), 1);
        executor.client().close().await;
    }
    Ok(())
}

#[tokio::test]
async fn public_child_mutation_is_revalidated_and_invalid_children_fail_before_reads() -> Result<()>
{
    let benign = json!({"op":"transfer","dest_ss58":DEST,"amount_tao":"0.000000000"});
    let (executor, chain, keys) = fixture().await?;
    let mut multisig = MultisigThreshold1 {
        other_signatories: vec![json!(OTHER)],
        call: benign.clone(),
    };
    assert_eq!(multisig.safety()?.spend, Spend::Bounded(Balance::ZERO));
    multisig.call = json!({"op":"transfer_all","dest_ss58":DEST});
    assert_eq!(multisig.safety()?.spend, Spend::Unbounded);
    let opts = ExecuteOptions {
        policy: Some(Policy {
            max_spend: Some(Balance::ZERO),
            ..Default::default()
        }),
        ..pool()
    };
    assert!(matches!(
        executor.execute_dyn(&multisig, &keys, &opts).await,
        Err(TxError::Policy(_))
    ));
    let mut announced = ExecuteProxyAnnounced {
        delegate_ss58: OTHER.into(),
        real_ss58: DEST.into(),
        inner_op: "transfer".into(),
        inner_args: benign.clone(),
        force_proxy_type: None,
    };
    let mut batch = Batch::new(vec![threshold_one(benign)])?;
    for invalid in [
        Value::Null,
        json!({"op":"not_registered"}),
        json!({"op":"transfer","dest_ss58":DEST,"amount_tao":-1}),
        json!({"op":"set_subnet_emission_enabled","netuids":[],"enabled":true}),
    ] {
        let requests = chain.requests().len();
        multisig.call = invalid.clone();
        assert!(
            Policy::default()
                .check(&multisig, None)
                .iter()
                .any(|message| message.contains("raw call"))
        );
        assert!(
            executor
                .execute_dyn(&multisig, &keys, &pool())
                .await
                .is_err()
        );
        announced.inner_op = "multisig_threshold_1".into();
        announced.inner_args = threshold_one(invalid.clone());
        assert!(
            executor
                .execute_dyn(&announced, &keys, &pool())
                .await
                .is_err()
        );
        batch.intents[0] = proxy(threshold_one(invalid));
        assert!(executor.execute_dyn(&batch, &keys, &pool()).await.is_err());
        assert_eq!(chain.requests().len(), requests);
        no_private_or_submit(&chain, &keys);
    }
    executor.client().close().await;
    Ok(())
}
