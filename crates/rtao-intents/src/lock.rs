// Derived from RaoFoundation/subtensor sdk/python/bittensor/intents/lock.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/intents/staking.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Stake locks and conviction, including unlocked-alpha resolution.

use rtao_client::{View, ViewExt};
use rtao_core::{codec::Value, value_ext::ValueExt};
use rtao_primitives::{AlphaBalance, Amount, NetUid, ResolvedAmount, UnitError};
use rtao_runtime::runtime_apis::stake_info_runtime_api;
use rtao_tx::{BuildCx, Built, Intent, IntentError, PreflightCx};

pub use crate::generated::{LockStake, MoveLock, SetPerpetualLock, SetRejectLockedAlpha};

/// Resolve a lock top-up from the runtime's coldkey-wide unlocked stake.
pub(crate) async fn lockable_amount<V: View + ?Sized>(
    intent: &LockStake,
    cx: &BuildCx<'_, V>,
) -> Result<AlphaBalance, IntentError> {
    match intent.amount_alpha.resolve(intent.netuid, true)? {
        // Root alpha is TAO 1:1; the wire parameter is still AlphaBalance.
        ResolvedAmount::Exact(amount) => Ok(AlphaBalance(amount.rao)),
        ResolvedAmount::All => {
            let coldkey = cx.coldkey_address()?;
            let raw = cx
                .view
                .call(
                    &stake_info_runtime_api::GET_STAKE_AVAILABILITY_FOR_COLDKEYS,
                    (vec![coldkey], Some(vec![intent.netuid])),
                )
                .await?;
            let available = available_rao(
                raw.as_core(),
                &coldkey.to_ss58(cx.runtime.core().ss58_format),
                intent.netuid,
            )?;
            if available == 0 {
                return Err(IntentError::validation(format!(
                    "nothing to lock: no unlocked stake on netuid {}",
                    intent.netuid
                )));
            }
            Ok(AlphaBalance(available))
        }
    }
}

// Match _availability_rao: an absent coldkey/subnet is zero; prefer the native
// integer subnet key, then the string-key fallback used by Python transports.
fn available_rao(raw: &Value, coldkey: &str, netuid: NetUid) -> Result<u64, IntentError> {
    let row = raw.field(coldkey).and_then(|rows| {
        let Value::Dict(entries) = rows else {
            return None;
        };
        entries
            .iter()
            .find(|(key, _)| key.as_u128() == Some(u128::from(netuid.0)))
            .map(|(_, value)| value)
            .filter(|value| {
                !matches!(value, Value::Null)
                    && !matches!(value, Value::Dict(entries) if entries.is_empty())
            })
            .or_else(|| rows.field(&netuid.to_string()))
    });
    let available = row.and_then(|row| row.field("available"));
    match available {
        None | Some(Value::Null) => Ok(0),
        Some(available) => available
            .as_u128()
            .and_then(|value| u64::try_from(value).ok())
            .ok_or_else(|| UnitError::Overflow.into()),
    }
}

impl Intent for LockStake {
    fn summary(&self) -> String {
        let amount = match self.amount_alpha.resolve(self.netuid, true) {
            Ok(ResolvedAmount::All) => "ALL unlocked alpha".into(),
            Ok(ResolvedAmount::Exact(amount)) => amount.to_string(),
            // Constructor validation rejects mismatched typed currencies.
            Err(_) => match &self.amount_alpha {
                Amount::All => "ALL unlocked alpha".into(),
                Amount::Exact(amount) => amount.to_decimal_string(),
            },
        };
        format!("lock {amount} on netuid {}", self.netuid)
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        self.validate()?;
        crate::generated::passthrough::lock_stake(self, cx).await
    }

    async fn warnings<V: View + ?Sized>(
        &self,
        _cx: &PreflightCx<'_, V>,
    ) -> Result<Vec<String>, IntentError> {
        Ok(if matches!(self.amount_alpha, Amount::All) {
            vec!["locks every unlocked alpha this coldkey holds on the subnet".into()]
        } else {
            vec![]
        })
    }

    fn validate(&self) -> Result<(), IntentError> {
        self.amount_alpha.resolve(self.netuid, true)?;
        Ok(())
    }
}

impl Intent for MoveLock {
    fn summary(&self) -> String {
        format!(
            "move lock on netuid {} to {}",
            self.netuid, self.destination_hotkey_ss58
        )
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        crate::generated::passthrough::move_lock(self, cx).await
    }
}

impl Intent for SetPerpetualLock {
    fn summary(&self) -> String {
        let mode = if self.enabled {
            "perpetual"
        } else {
            "decaying"
        };
        format!("set lock mode to {mode} on netuid {}", self.netuid)
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        crate::generated::passthrough::set_perpetual_lock(self, cx).await
    }
}

impl Intent for SetRejectLockedAlpha {
    fn summary(&self) -> String {
        let action = if self.enabled { "reject" } else { "accept" };
        format!("{action} incoming locked alpha transfers")
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        crate::generated::passthrough::set_reject_locked_alpha(self, cx).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rtao_client::{BlockRef, Client, ClientConfig, DynView, Network, Snapshot};
    use rtao_primitives::{AccountId, DynBalance, Money, Spend};
    use rtao_runtime::{CallBytes, RuntimeHandle};
    use rtao_signer::{KeyView, SignerError};
    use rtao_testkit::{FakeChain, FakeState, rpc::RpcClient};
    use rtao_tx::{IntentInterface, OriginView, SystemClock};
    use serde_json::{Value as JsonValue, json};

    const COLDKEY: AccountId = AccountId([7; 32]);
    const HOTKEY: AccountId = AccountId([8; 32]);
    const OTHER_COLDKEY: AccountId = AccountId([9; 32]);
    const NETUID: NetUid = NetUid(11);
    const AVAILABILITY_METHOD: &str = "StakeInfoRuntimeApi_get_stake_availability_for_coldkeys";

    struct PublicKeys;
    impl KeyView for PublicKeys {
        fn hotkey_address(&self) -> Result<AccountId, SignerError> {
            Ok(HOTKEY)
        }
        fn hotkey_public_key(&self) -> Result<[u8; 32], SignerError> {
            Ok(HOTKEY.0)
        }
        fn coldkey_address(&self) -> Result<AccountId, SignerError> {
            Ok(COLDKEY)
        }
    }

    async fn fixture(raw: JsonValue) -> anyhow::Result<(FakeChain, Client, Snapshot)> {
        let mut state = FakeState::try_default()?;
        // JSON object keys are strings; this native nested map uses integer
        // NetUid keys. Encode those keys explicitly through pinned metadata.
        let mut value = rtao_testkit::fakes::json_to_core(&raw)?;
        if let Value::Dict(coldkeys) = &mut value {
            for (_, rows) in coldkeys {
                if let Value::Dict(entries) = rows {
                    for (key, _) in entries {
                        if let Value::Str(text) = key {
                            *key = Value::Int(i128::from(text.parse::<u16>()?));
                        }
                    }
                }
            }
        }
        let api = "StakeInfoRuntimeApi";
        let method = "get_stake_availability_for_coldkeys";
        let signature =
            rtao_core::ops::runtime_api::runtime_api_method(&state.runtime, api, method)?;
        let output = state
            .runtime
            .encode_spec(&rtao_core::runtime::TypeSpec::Id(signature.output), &value)?;
        let mut inputs = Vec::new();
        for coldkey in [COLDKEY, OTHER_COLDKEY] {
            for netuid in [NetUid::ROOT, NETUID, NetUid(12)] {
                let input = rtao_core::ops::runtime_api::encode_params(
                    &state.runtime,
                    api,
                    signature,
                    &[
                        Value::List(vec![Value::Bytes(coldkey.0.to_vec())]),
                        Value::List(vec![Value::Int(i128::from(netuid.0))]),
                    ],
                )?;
                inputs.push(input);
            }
        }
        for input in inputs {
            state.seed_runtime_encoded(AVAILABILITY_METHOD, input, output.clone());
        }
        let chain = FakeChain::from_state(state);
        let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
        config.track_heads = false;
        config.runtime_cache_dir = None;
        config.config_store = None;
        let client = Client::from_rpc(RpcClient::new(chain.clone()), config).await?;
        let snapshot = client.at(BlockRef::Finalized).await?;
        Ok((chain, client, snapshot))
    }

    fn availability(coldkey: AccountId, netuid: NetUid, available: u64) -> JsonValue {
        // Deliberately not total - locked: use the runtime's available value.
        json!({(coldkey.to_string()): {(netuid.to_string()): {
            "total": 1000, "locked": 400, "available": available
        }}})
    }

    fn decode(call: &CallBytes, runtime: &RuntimeHandle) -> anyhow::Result<JsonValue> {
        let decoded = runtime
            .core()
            .decode_spec(&rtao_core::runtime::TypeSpec::Call, call.as_bytes(), true)
            .map_err(rtao_core::CoreError::from)?;
        Ok(rtao_core::codec::value::to_corpus_json(&decoded))
    }

    fn expected_lock(hotkey: AccountId, netuid: NetUid, amount: u64) -> Vec<u8> {
        // Pinned dispatch 136 has AccountId32, NetUid and non-compact AlphaBalance.
        let mut bytes = vec![7, 136];
        bytes.extend_from_slice(hotkey.as_bytes());
        bytes.extend_from_slice(&netuid.0.to_le_bytes());
        bytes.extend_from_slice(&amount.to_le_bytes());
        bytes
    }

    #[test]
    fn lock_constructor_preserves_exact_native_amounts_defaults_and_units() -> anyhow::Result<()> {
        for (input, output) in [
            (json!(0), json!("0")),
            (json!("0.000000001"), json!("0.000000001")),
            (
                json!("18446744073.709551615"),
                json!("18446744073.709551615"),
            ),
            (json!(" ALL "), json!("all")),
        ] {
            let intent = crate::AnyIntent::from_tool(
                "lock_stake",
                json!({"netuid":11,"amount_alpha":input}),
            )?;
            assert_eq!(
                intent.to_json(),
                json!({
                    "op":"lock_stake","netuid":11,"amount_alpha":output,"hotkey_ss58":null
                })
            );
            assert_eq!(
                crate::AnyIntent::from_tool("lock_stake", intent.to_json())?,
                intent
            );
        }
        for input in [
            json!("18446744073.709551616"),
            json!("-0.000000001"),
            json!(null),
            json!(true),
            json!("NaN"),
            json!("Infinity"),
        ] {
            assert!(
                crate::AnyIntent::from_tool(
                    "lock_stake",
                    json!({"netuid":11,"amount_alpha":input})
                )
                .is_err()
            );
        }
        for (op, args) in [
            ("lock_stake", json!({"netuid":11})),
            ("lock_stake", json!({"amount_alpha":"all"})),
            (
                "lock_stake",
                json!({"netuid":11,"amount_alpha":1,"extra":true}),
            ),
            ("set_perpetual_lock", json!({"netuid":11})),
            ("set_perpetual_lock", json!({"enabled":true})),
            ("set_perpetual_lock", json!({"netuid":11,"enabled":null})),
            ("set_reject_locked_alpha", json!({})),
            (
                "set_reject_locked_alpha",
                json!({"enabled":true,"extra":true}),
            ),
            ("move_lock", json!({"netuid":11})),
            (
                "move_lock",
                json!({"netuid":11,"destination_hotkey_ss58":"invalid"}),
            ),
        ] {
            assert!(crate::AnyIntent::from_tool(op, args).is_err(), "{op}");
        }
        let mut intent = LockStake {
            netuid: NETUID,
            amount_alpha: DynBalance::from_rao(1, NetUid(12)).into(),
            hotkey_ss58: None,
        };
        assert!(matches!(
            intent.validate(),
            Err(IntentError::UnitMismatch(UnitError::UnitMismatch {
                expected: NETUID,
                actual: NetUid(12)
            }))
        ));
        intent.amount_alpha = DynBalance::from_rao(u64::MAX, NETUID).into();
        intent.validate()?;
        intent.netuid = NetUid::ROOT;
        intent.amount_alpha = DynBalance::from_rao(u64::MAX, NetUid::ROOT).into();
        intent.validate()?;
        Ok(())
    }

    #[tokio::test]
    async fn lock_exact_encodes_native_boundaries_root_and_account_prefixes_without_reads()
    -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture(json!({})).await?;
        let runtime = &snapshot.block().runtime;
        let before = chain.requests().len();
        for netuid in [NetUid::ROOT, NETUID] {
            for amount in [0, 1, u64::MAX] {
                for prefix in [0, 2, 42, 110] {
                    let intent = LockStake {
                        netuid,
                        amount_alpha: Money::from_rao(amount).into(),
                        hotkey_ss58: Some(HOTKEY.to_ss58(prefix).parse()?),
                    };
                    let built = intent
                        .build(&BuildCx {
                            view: &snapshot,
                            origin: OriginView::Account(COLDKEY),
                            runtime,
                            clock: &SystemClock,
                            spec: LockStake::SPEC,
                        })
                        .await?;
                    let expected = expected_lock(HOTKEY, netuid, amount);
                    assert_eq!(built.call.as_bytes(), expected);
                    assert_eq!(CallBytes::decode(&expected, runtime)?, built.call);
                    assert!(built.extras.is_empty());
                    let decoded = decode(&built.call, runtime)?;
                    assert_eq!(decoded["call_module"], "SubtensorModule");
                    assert_eq!(decoded["call_function"], "lock_stake");
                    assert_eq!(decoded["call_args"][0]["value"], HOTKEY.to_string());
                    assert_eq!(decoded["call_args"][1]["value"], netuid.0);
                    assert_eq!(decoded["call_args"][2]["value"], amount);
                }
            }
        }
        let intent = LockStake {
            netuid: NETUID,
            amount_alpha: Money::from_rao(1).into(),
            hotkey_ss58: None,
        };
        let built = intent
            .build(&BuildCx {
                view: &snapshot,
                origin: OriginView::Keys(&PublicKeys),
                runtime,
                clock: &SystemClock,
                spec: LockStake::SPEC,
            })
            .await?;
        assert_eq!(built.call.as_bytes(), expected_lock(HOTKEY, NETUID, 1));
        assert_eq!(chain.requests().len(), before);
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn lock_all_uses_effective_coldkey_available_mass_and_one_pinned_runtime_read()
    -> anyhow::Result<()> {
        for (coldkey, netuid, amount) in [
            (COLDKEY, NETUID, 1),
            (OTHER_COLDKEY, NETUID, u64::MAX),
            (COLDKEY, NetUid::ROOT, 42),
        ] {
            let (chain, client, snapshot) = fixture(availability(coldkey, netuid, amount)).await?;
            let runtime = &snapshot.block().runtime;
            let before = chain.requests().len();
            let intent = LockStake {
                netuid,
                amount_alpha: Amount::All,
                hotkey_ss58: Some(HOTKEY.into()),
            };
            let built = intent
                .build(&BuildCx {
                    view: &snapshot,
                    origin: OriginView::Account(coldkey),
                    runtime,
                    clock: &SystemClock,
                    spec: LockStake::SPEC,
                })
                .await?;
            assert_eq!(built.call.as_bytes(), expected_lock(HOTKEY, netuid, amount));
            let requests = chain.requests();
            let reads = &requests[before..];
            assert_eq!(reads.len(), 1);
            assert_eq!(reads[0].method, "state_call");
            assert_eq!(reads[0].params[0], AVAILABILITY_METHOD);
            // Vec<AccountId32>(one), Some(Vec<NetUid>(one)), then netuid u16.
            let mut input = vec![4];
            input.extend_from_slice(coldkey.as_bytes());
            input.extend_from_slice(&[1, 4]);
            input.extend_from_slice(&netuid.0.to_le_bytes());
            assert_eq!(reads[0].params[1], format!("0x{}", hex::encode(input)));
            assert_eq!(
                reads[0].params[2],
                format!("0x{}", hex::encode(snapshot.block().hash.0))
            );
            assert!(built.extras.is_empty());
            assert!(chain.mutate_state(|state| state.submissions.is_empty()));
            client.close().await;
        }
        Ok(())
    }

    #[tokio::test]
    async fn lock_all_propagates_malformed_runtime_data_without_composing_or_submitting()
    -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture(availability(COLDKEY, NETUID, 600)).await?;
        // Empty bytes cannot encode the runtime's map, including its length.
        chain.mutate_state(|state| state.set_result("state_call", json!("0x")));
        let before = chain.requests().len();
        let intent = LockStake {
            netuid: NETUID,
            amount_alpha: Amount::All,
            hotkey_ss58: None,
        };
        let result = intent
            .build(&BuildCx {
                view: &snapshot,
                origin: OriginView::Keys(&PublicKeys),
                runtime: &snapshot.block().runtime,
                clock: &SystemClock,
                spec: LockStake::SPEC,
            })
            .await;
        assert!(matches!(result, Err(IntentError::Client(_))));
        assert_eq!(chain.requests().len(), before + 1);
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn lock_all_refuses_zero_missing_and_other_rows_and_resolves_hotkey_before_reads()
    -> anyhow::Result<()> {
        for raw in [
            json!({}),
            availability(COLDKEY, NETUID, 0),
            availability(OTHER_COLDKEY, NETUID, 100),
            availability(COLDKEY, NetUid(12), 100),
        ] {
            let (chain, client, snapshot) = fixture(raw).await?;
            let runtime = &snapshot.block().runtime;
            let mut intent = LockStake {
                netuid: NETUID,
                amount_alpha: Amount::All,
                hotkey_ss58: None,
            };
            let before = chain.requests().len();
            let error = intent
                .build(&BuildCx {
                    view: &snapshot,
                    origin: OriginView::Account(COLDKEY),
                    runtime,
                    clock: &SystemClock,
                    spec: LockStake::SPEC,
                })
                .await
                .err()
                .ok_or_else(|| anyhow::anyhow!("missing hotkey accepted"))?;
            assert_eq!(
                error.to_string(),
                "lock_stake: this signer carries no hotkey to default to; pass the hotkey address explicitly or sign with a Wallet"
            );
            assert_eq!(chain.requests().len(), before);
            intent.hotkey_ss58 = Some(HOTKEY.into());
            let error = intent
                .build(&BuildCx {
                    view: &snapshot,
                    origin: OriginView::Account(COLDKEY),
                    runtime,
                    clock: &SystemClock,
                    spec: LockStake::SPEC,
                })
                .await
                .err()
                .ok_or_else(|| anyhow::anyhow!("empty available stake accepted"))?;
            assert_eq!(
                error.to_string(),
                "nothing to lock: no unlocked stake on netuid 11"
            );
            assert_eq!(chain.requests().len(), before + 1);
            client.close().await;
        }
        Ok(())
    }

    #[tokio::test]
    async fn lock_settings_and_move_encode_exact_calls_without_policy_or_extra_reads()
    -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture(json!({})).await?;
        let runtime = &snapshot.block().runtime;
        let before = chain.requests().len();
        for (op, args, expected, summary, touched) in [
            (
                "set_perpetual_lock",
                json!({"netuid":11,"enabled":true}),
                vec![7, 138, 11, 0, 1],
                "set lock mode to perpetual on netuid 11".into(),
                vec![NETUID],
            ),
            (
                "set_perpetual_lock",
                json!({"netuid":11,"enabled":false}),
                vec![7, 138, 11, 0, 0],
                "set lock mode to decaying on netuid 11".into(),
                vec![NETUID],
            ),
            (
                "set_reject_locked_alpha",
                json!({"enabled":true}),
                vec![7, 142, 1],
                "reject incoming locked alpha transfers".into(),
                vec![],
            ),
            (
                "set_reject_locked_alpha",
                json!({"enabled":false}),
                vec![7, 142, 0],
                "accept incoming locked alpha transfers".into(),
                vec![],
            ),
            (
                "move_lock",
                json!({"netuid":11,"destination_hotkey_ss58":HOTKEY.to_ss58(110)}),
                {
                    let mut bytes = vec![7, 137];
                    bytes.extend_from_slice(HOTKEY.as_bytes());
                    bytes.extend_from_slice(&[11, 0]);
                    bytes
                },
                format!("move lock on netuid 11 to {}", HOTKEY.to_ss58(110)),
                vec![NETUID],
            ),
        ] {
            let intent = crate::AnyIntent::from_tool(op, args)?;
            let dynamic = intent.as_dyn();
            assert_eq!(dynamic.summary_dyn(), summary);
            let built = dynamic
                .build_dyn(&BuildCx {
                    view: &snapshot as &dyn DynView,
                    origin: OriginView::Account(COLDKEY),
                    runtime,
                    clock: &SystemClock,
                    spec: dynamic.spec(),
                })
                .await?;
            assert_eq!(built.call.as_bytes(), expected);
            assert_eq!(CallBytes::decode(&expected, runtime)?, built.call);
            assert_eq!(decode(&built.call, runtime)?["call_function"], op);
            assert!(built.extras.is_empty());
            let preview = dynamic
                .preflight_dyn(&PreflightCx {
                    view: &snapshot as &dyn DynView,
                    runtime,
                    clock: &SystemClock,
                    spec: dynamic.spec(),
                    client: &client,
                    dispatch_origin: COLDKEY,
                    fee_payer: OTHER_COLDKEY,
                    call: &built.call,
                })
                .await?;
            assert_eq!(preview.effects, vec![summary]);
            assert!(preview.warnings.is_empty());
            assert!(preview.blocks.is_empty());
            assert!(preview.facts.is_empty());
            assert!(preview.required_free.is_none());
            assert!(preview.available_free.is_none());
            assert!(preview.estimated_fee.is_none());
            assert_eq!(dynamic.spend_dyn(), Spend::None);
            assert_eq!(dynamic.touches_netuids_dyn().as_slice(), touched.as_slice());
            assert!(!dynamic.affects_all_subnets_dyn());
            assert_eq!(dynamic.spec().signer, "coldkey");
            assert_eq!(dynamic.spec().origin, "signed");
            assert_eq!(
                crate::AnyIntent::from_tool(op, dynamic.to_json_dyn())?,
                intent
            );
        }
        assert_eq!(chain.requests().len(), before);
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn lock_preflight_preserves_exact_summaries_all_warning_and_no_spend()
    -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture(availability(COLDKEY, NETUID, 600)).await?;
        let runtime = &snapshot.block().runtime;
        for (amount, summary, warnings) in [
            (
                Money::from_rao(1).into(),
                "lock 0.000000001α₁₁ on netuid 11",
                vec![],
            ),
            (
                Amount::All,
                "lock ALL unlocked alpha on netuid 11",
                vec!["locks every unlocked alpha this coldkey holds on the subnet"],
            ),
        ] {
            let intent = LockStake {
                netuid: NETUID,
                amount_alpha: amount,
                hotkey_ss58: Some(HOTKEY.into()),
            };
            let built = intent
                .build(&BuildCx {
                    view: &snapshot,
                    origin: OriginView::Account(COLDKEY),
                    runtime,
                    clock: &SystemClock,
                    spec: LockStake::SPEC,
                })
                .await?;
            let before = chain.requests().len();
            let preview = intent
                .preflight(&PreflightCx {
                    view: &snapshot,
                    runtime,
                    clock: &SystemClock,
                    spec: LockStake::SPEC,
                    client: &client,
                    dispatch_origin: COLDKEY,
                    fee_payer: OTHER_COLDKEY,
                    call: &built.call,
                })
                .await?;
            assert_eq!(intent.summary(), summary);
            assert_eq!(preview.effects, vec![summary]);
            assert_eq!(preview.warnings, warnings);
            assert!(preview.blocks.is_empty());
            assert!(preview.facts.is_empty());
            assert!(preview.required_free.is_none());
            assert!(preview.available_free.is_none());
            assert!(preview.estimated_fee.is_none());
            assert_eq!(intent.spend(), Spend::None);
            assert_eq!(intent.touches_netuids().as_slice(), &[NETUID]);
            assert!(!intent.affects_all_subnets());
            assert_eq!(LockStake::SPEC.all_amount_fields, &["amount_alpha"]);
            assert_eq!(chain.requests().len(), before);
        }
        client.close().await;
        Ok(())
    }

    #[test]
    fn lock_availability_keeps_integer_key_precedence_string_fallback_and_native_range()
    -> anyhow::Result<()> {
        let entry = |available| Value::record(vec![("available".into(), available)]);
        let raw = |integer, string| {
            Value::record(vec![(
                COLDKEY.to_string(),
                Value::Dict(vec![
                    (Value::Int(11), integer),
                    (Value::Str("11".into()), string),
                ]),
            )])
        };
        assert_eq!(
            available_rao(
                &raw(entry(Value::Int(7)), entry(Value::Int(8))),
                &COLDKEY.to_string(),
                NETUID
            )?,
            7
        );
        assert_eq!(
            available_rao(
                &raw(Value::Dict(vec![]), entry(Value::Int(8))),
                &COLDKEY.to_string(),
                NETUID
            )?,
            8
        );
        assert_eq!(
            available_rao(
                &raw(Value::Null, entry(Value::Int(8))),
                &COLDKEY.to_string(),
                NETUID
            )?,
            8
        );
        assert!(
            available_rao(
                &raw(entry(Value::Uint(u128::from(u64::MAX) + 1)), Value::Null),
                &COLDKEY.to_string(),
                NETUID
            )
            .is_err()
        );
        Ok(())
    }
}
