// Derived from RaoFoundation/subtensor sdk/python/bittensor/intents/collateral.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Additional source at the same commit/license: sdk/python/bittensor/intents/staking.py.
//! Miner collateral top-ups and the emission-funded collateral floor.

use rtao_client::{View, ViewExt};
use rtao_primitives::{AlphaBalance, Amount, NetUid, PriceRao, ResolvedAmount, Spend, UnitError};
use rtao_runtime::{calls::subtensor_module, runtime_apis, storage::subtensor_module as st};
use rtao_tx::{BuildCx, Built, Intent, IntentError, PreflightCx};

pub use crate::{AddCollateral, SetMinCollateral};

fn amount_summary(amount: &Amount, netuid: NetUid) -> String {
    match amount {
        Amount::All => "ALL free alpha".into(),
        Amount::Exact(money) => money
            .resolve(netuid)
            .map(|amount| amount.to_string())
            .unwrap_or_else(|_| money.to_decimal_string()),
    }
}

fn check_rate_tolerance(tolerance: f64) -> Result<(), IntentError> {
    if !(0.0..1.0).contains(&tolerance) {
        return Err(IntentError::Validation(format!(
            "rate_tolerance must be a fraction in [0, 1), got {} (0.05 = 5%). To trade with no price bound, disable slippage protection (slippage_protection=False / --no-slippage-protection) instead.",
            rtao_primitives::pyfmt::float_repr(tolerance)
        )));
    }
    Ok(())
}

fn limit_price(price: u64, tolerance: f64) -> Result<PriceRao, IntentError> {
    // Python converts price to binary64 before multiplication, then truncates.
    // Test the exclusive 2^64 boundary before the cast: u64::MAX as f64 rounds
    // up, and Rust's saturating float cast would silently change that limit.
    let limit = (price as f64 * (1.0 + tolerance)).trunc();
    if !(0.0..18_446_744_073_709_551_616.0).contains(&limit) {
        return Err(UnitError::Overflow.into());
    }
    Ok(PriceRao(limit as u64))
}

impl Intent for AddCollateral {
    fn summary(&self) -> String {
        format!(
            "lock {} as collateral on netuid {} (fails if price moves >{:.2}%)",
            amount_summary(&self.amount_alpha, self.netuid),
            self.netuid,
            self.rate_tolerance.get() * 100.0,
        )
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        self.validate()?;
        let hotkey = cx.hotkey_address(self.hotkey_ss58.as_ref().map(|key| key.account()))?;
        let alpha = match self.amount_alpha.resolve(self.netuid, true)? {
            ResolvedAmount::Exact(amount) => amount.rao,
            ResolvedAmount::All => {
                let coldkey = cx.coldkey_address()?;
                let snapshot = cx.view.pin().await?;
                let (info, collateral) = tokio::try_join!(
                    snapshot.call(
                        &runtime_apis::stake_info_runtime_api::GET_STAKE_INFO_FOR_HOTKEY_COLDKEY_NETUID,
                        (hotkey, coldkey, self.netuid),
                    ),
                    snapshot.storage(&st::MINER_COLLATERAL, (self.netuid, hotkey, coldkey)),
                )?;
                let stake = info.map_or(0, |info| info.stake.0);
                let locked = collateral.map_or(0, |state| state.locked.0);
                stake.checked_sub(locked).filter(|free| *free > 0).ok_or_else(|| {
                    let spelling = self.hotkey_ss58.as_ref().map_or_else(
                        || hotkey.to_ss58(42),
                        |key| key.as_str().to_owned(),
                    );
                    IntentError::Validation(format!(
                        "nothing to lock as collateral: no free alpha on {spelling} at netuid {}",
                        self.netuid
                    ))
                })?
            }
        };
        let price = cx
            .view
            .call(
                &runtime_apis::swap_runtime_api::CURRENT_ALPHA_PRICE,
                (self.netuid,),
            )
            .await?;
        Ok(Built::new(
            subtensor_module::add_collateral(
                self.netuid,
                hotkey,
                // Root operations retain TAO's netuid-zero denomination, but
                // the call's wire newtype remains AlphaBalance.
                AlphaBalance(alpha),
                limit_price(price, self.rate_tolerance.get())?,
            )
            .compose(cx.runtime)?,
        ))
    }

    fn validate(&self) -> Result<(), IntentError> {
        // Match Python's constructor order: currency/all before tolerance.
        self.amount_alpha.resolve(self.netuid, true)?;
        check_rate_tolerance(self.rate_tolerance.get())
    }

    async fn warnings<V: View + ?Sized>(
        &self,
        _cx: &PreflightCx<'_, V>,
    ) -> Result<Vec<String>, IntentError> {
        Ok(if self.amount_alpha == Amount::All {
            vec!["locks every free (not already collateralized) alpha on this hotkey".into()]
        } else {
            vec![]
        })
    }

    fn spend(&self) -> Spend {
        // Even an exact alpha request can buy a TAO shortfall at dispatch.
        Spend::Unbounded
    }
}

impl Intent for SetMinCollateral {
    fn summary(&self) -> String {
        format!(
            "set collateral floor to {} on netuid {}",
            amount_summary(&self.min_alpha, self.netuid),
            self.netuid,
        )
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        self.validate()?;
        let hotkey = cx.hotkey_address(self.hotkey_ss58.as_ref().map(|key| key.account()))?;
        let ResolvedAmount::Exact(amount) = self.min_alpha.resolve(self.netuid, false)? else {
            return Err(UnitError::AllNotAllowed.into());
        };
        Ok(Built::new(
            subtensor_module::set_min_collateral(self.netuid, hotkey, AlphaBalance(amount.rao))
                .compose(cx.runtime)?,
        ))
    }

    fn validate(&self) -> Result<(), IntentError> {
        self.min_alpha.resolve(self.netuid, false)?;
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use rtao_client::{BlockRef, Client, ClientConfig, DynView, Network, Snapshot};
    use rtao_primitives::{AccountId, DynBalance, Money, Ratio};
    use rtao_runtime::{CallBytes, RuntimeHandle};
    use rtao_signer::{KeyView, SignerError};
    use rtao_testkit::{FakeChain, FakeState, RecordingView, rpc::RpcClient};
    use rtao_tx::{DynIntent, IntentInterface, OriginView, SystemClock};
    use serde_json::{Value, json};

    const OWNER: AccountId = AccountId([7; 32]);
    const HOTKEY: AccountId = AccountId([8; 32]);
    const NETUID: NetUid = NetUid(513);

    struct PublicKeys;
    impl KeyView for PublicKeys {
        fn hotkey_address(&self) -> Result<AccountId, SignerError> {
            Ok(HOTKEY)
        }
        fn hotkey_public_key(&self) -> Result<[u8; 32], SignerError> {
            Ok(HOTKEY.0)
        }
        fn coldkey_address(&self) -> Result<AccountId, SignerError> {
            Ok(OWNER)
        }
    }

    async fn fixture(
        setup: impl FnOnce(&mut FakeState) -> anyhow::Result<()>,
    ) -> anyhow::Result<(FakeChain, Client, Snapshot)> {
        let mut state = FakeState::try_default()?;
        setup(&mut state)?;
        let chain = FakeChain::from_state(state);
        let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
        config.track_heads = false;
        config.runtime_cache_dir = None;
        config.config_store = None;
        let client = Client::from_rpc(RpcClient::new(chain.clone()), config).await?;
        let snapshot = client.at(BlockRef::Finalized).await?;
        Ok((chain, client, snapshot))
    }

    fn add(amount: Amount) -> anyhow::Result<AddCollateral> {
        Ok(AddCollateral {
            netuid: NETUID,
            amount_alpha: amount,
            hotkey_ss58: Some(HOTKEY.to_ss58(2).parse()?),
            rate_tolerance: Ratio::new(0.05)?,
        })
    }

    fn expected_call(index: u8, netuid: NetUid, alpha: u64, price: Option<u64>) -> Vec<u8> {
        let mut expected = vec![7, index];
        expected.extend_from_slice(&netuid.0.to_le_bytes());
        expected.extend_from_slice(&HOTKEY.0);
        expected.extend_from_slice(&alpha.to_le_bytes());
        if let Some(price) = price {
            expected.extend_from_slice(&price.to_le_bytes());
        }
        expected
    }

    fn decoded(call: &CallBytes, runtime: &RuntimeHandle) -> anyhow::Result<Value> {
        let value = runtime
            .core()
            .decode_spec(&rtao_core::runtime::TypeSpec::Call, call.as_bytes(), true)
            .map_err(rtao_core::CoreError::from)?;
        Ok(rtao_core::codec::value::to_corpus_json(&value))
    }

    #[test]
    fn collateral_amounts_keep_native_units_bounds_json_and_constructor_order() -> anyhow::Result<()>
    {
        for netuid in [NetUid::ROOT, NETUID] {
            for rao in [0, 1, 1_000_000_001, u64::MAX] {
                let amount: Amount = DynBalance::from_rao(rao, netuid).into();
                let mut intent = add(amount.clone())?;
                intent.netuid = netuid;
                intent.validate()?;
                let floor = SetMinCollateral {
                    netuid,
                    min_alpha: amount,
                    hotkey_ss58: None,
                };
                floor.validate()?;
                for (op, json) in [
                    ("add_collateral", intent.to_json()),
                    ("set_min_collateral", floor.to_json()),
                ] {
                    assert_eq!(
                        crate::AnyIntent::from_tool(op, json.clone())?.to_json(),
                        json
                    );
                }
            }
        }
        let intent = add(Money::from_rao(1_000_000_001).into())?;
        assert_eq!(
            intent.summary(),
            "lock 1.000000001α₅₁₃ as collateral on netuid 513 (fails if price moves >5.00%)"
        );
        let floor = SetMinCollateral {
            netuid: NETUID,
            min_alpha: Money::from_rao(0).into(),
            hotkey_ss58: None,
        };
        assert_eq!(
            floor.summary(),
            "set collateral floor to 0.000000000α₅₁₃ on netuid 513"
        );
        let mut invalid = add(DynBalance::from_rao(1, NetUid::ROOT).into())?;
        invalid.rate_tolerance = Ratio::new(1.0)?;
        assert!(matches!(
            invalid.validate(),
            Err(IntentError::UnitMismatch(UnitError::UnitMismatch {
                expected: NETUID,
                actual: NetUid::ROOT,
            }))
        ));
        for tolerance in [-0.1, 1.0, 1.5] {
            invalid.amount_alpha = Amount::All;
            invalid.rate_tolerance = Ratio::new(tolerance)?;
            assert_eq!(
                invalid
                    .validate()
                    .expect_err("invalid tolerance")
                    .to_string(),
                format!(
                    "rate_tolerance must be a fraction in [0, 1), got {} (0.05 = 5%). To trade with no price bound, disable slippage protection (slippage_protection=False / --no-slippage-protection) instead.",
                    rtao_primitives::pyfmt::float_repr(tolerance)
                )
            );
        }
        let floor = SetMinCollateral {
            min_alpha: Amount::All,
            ..floor
        };
        assert!(matches!(
            floor.validate(),
            Err(IntentError::UnitMismatch(UnitError::AllNotAllowed))
        ));
        Ok(())
    }

    #[test]
    fn collateral_float_limit_keeps_python_round_then_truncate_and_refuses_overflow()
    -> anyhow::Result<()> {
        for (price, tolerance, expected) in [
            (0, 0.05, 0),
            (19, 0.05, 19),
            (20, 0.05, 21),
            (1_000_000_001, 0.05, 1_050_000_001),
            // Conversion rounds down at the first integer above 2^53.
            (9_007_199_254_740_993, 0.0, 9_007_199_254_740_992),
            // The next odd integer rounds up, rather than truncating the int first.
            (9_007_199_254_740_995, 0.0, 9_007_199_254_740_996),
        ] {
            assert_eq!(limit_price(price, tolerance)?.0, expected);
        }
        assert!(matches!(
            limit_price(u64::MAX, 0.0),
            Err(IntentError::UnitMismatch(UnitError::Overflow))
        ));
        assert!(matches!(
            limit_price(1_u64 << 63, 0.999_999_999_999_999_9),
            Err(IntentError::UnitMismatch(UnitError::Overflow))
        ));
        Ok(())
    }

    #[tokio::test]
    async fn collateral_exact_builds_read_only_price_and_preserve_scale_at_root_and_subnet()
    -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture(|state| {
            state.seed_runtime(
                "SwapRuntimeApi",
                "current_alpha_price",
                json!(1_000_000_001),
            )?;
            Ok(())
        })
        .await?;
        let runtime = &snapshot.block().runtime;
        let recorded = RecordingView::new(&snapshot);
        for netuid in [NetUid::ROOT, NETUID] {
            for rao in [0, 1, 0x0807_0605_0403_0201, u64::MAX] {
                recorded.clear();
                let mut intent = add(DynBalance::from_rao(rao, netuid).into())?;
                intent.netuid = netuid;
                let built = intent
                    .build(&BuildCx {
                        view: &recorded,
                        origin: OriginView::Account(OWNER),
                        runtime,
                        clock: &SystemClock,
                        spec: AddCollateral::SPEC,
                    })
                    .await?;
                let expected = expected_call(144, netuid, rao, Some(1_050_000_001));
                assert_eq!(built.call.as_bytes(), expected);
                assert_eq!(CallBytes::decode(&expected, runtime)?, built.call);
                assert!(built.extras.is_empty());
                let value = decoded(&built.call, runtime)?;
                assert_eq!(value["call_module"], "SubtensorModule");
                assert_eq!(value["call_function"], "add_collateral");
                assert_eq!(value["call_args"][0]["value"], netuid.0);
                assert_eq!(value["call_args"][2]["value"], rao);
                assert_eq!(value["call_args"][3]["value"], 1_050_000_001_u64);
                let touches = recorded.touches();
                assert_eq!(touches.len(), 1);
                assert_eq!(touches[0].item, "current_alpha_price");
                assert_eq!(recorded.block_hashes().len(), 1);
            }
        }
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn all_collateral_subtracts_miner_lock_at_effective_coldkey_before_pricing()
    -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture(|state| {
            state.seed_runtime(
                "StakeInfoRuntimeApi",
                "get_stake_info_for_hotkey_coldkey_netuid",
                json!({"stake":1_000_000_001,"locked":u64::MAX}),
            )?;
            state.seed_python(
                "SubtensorModule",
                "MinerCollateral",
                &[
                    json!(NETUID.0),
                    json!(HOTKEY.to_ss58(42)),
                    json!(OWNER.to_ss58(42)),
                ],
                json!({"locked":1}),
            )?;
            state.seed_runtime("SwapRuntimeApi", "current_alpha_price", json!(20))?;
            Ok(())
        })
        .await?;
        let runtime = &snapshot.block().runtime;
        let recorded = RecordingView::new(&snapshot);
        let intent = add(Amount::All)?;
        let requests = chain.requests().len();
        let built = intent
            .build(&BuildCx {
                view: &recorded,
                origin: OriginView::Account(OWNER),
                runtime,
                clock: &SystemClock,
                spec: AddCollateral::SPEC,
            })
            .await?;
        let expected = expected_call(144, NETUID, 1_000_000_000, Some(21));
        assert_eq!(built.call.as_bytes(), expected);
        assert_eq!(CallBytes::decode(&expected, runtime)?, built.call);
        assert!(built.extras.is_empty());
        let touches = recorded.touches();
        assert_eq!(touches.len(), 3);
        let mut free_reads = touches[..2]
            .iter()
            .map(|touch| touch.item.as_str())
            .collect::<Vec<_>>();
        free_reads.sort_unstable();
        assert_eq!(
            free_reads,
            [
                "MinerCollateral",
                "get_stake_info_for_hotkey_coldkey_netuid"
            ]
        );
        assert_eq!(touches[2].item, "current_alpha_price");
        assert_eq!(recorded.block_hashes().len(), 1);
        let mut stake_input = HOTKEY.0.to_vec();
        stake_input.extend_from_slice(&OWNER.0);
        stake_input.extend_from_slice(&NETUID.0.to_le_bytes());
        let calls = chain.requests()[requests..]
            .iter()
            .filter(|request| request.method == "state_call")
            .map(|request| request.params.clone())
            .collect::<Vec<_>>();
        assert_eq!(calls.len(), 2);
        assert_eq!(
            calls[0][0],
            "StakeInfoRuntimeApi_get_stake_info_for_hotkey_coldkey_netuid"
        );
        assert_eq!(calls[0][1], format!("0x{}", hex::encode(stake_input)));
        assert_eq!(calls[1][0], "SwapRuntimeApi_current_alpha_price");
        assert_eq!(calls[1][1], "0x0102");
        assert_eq!(calls[0][2], calls[1][2]);
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn all_collateral_refuses_empty_missing_and_overlocked_before_price_read()
    -> anyhow::Result<()> {
        for (stake, locked) in [
            (None, None),
            (Some(0), None),
            (Some(10), Some(10)),
            (Some(10), Some(11)),
        ] {
            let (_, client, snapshot) = fixture(|state| {
                state.seed_runtime(
                    "StakeInfoRuntimeApi",
                    "get_stake_info_for_hotkey_coldkey_netuid",
                    stake.map_or(Value::Null, |stake| json!({"stake":stake})),
                )?;
                if let Some(locked) = locked {
                    state.seed_python(
                        "SubtensorModule",
                        "MinerCollateral",
                        &[
                            json!(NETUID.0),
                            json!(HOTKEY.to_ss58(42)),
                            json!(OWNER.to_ss58(42)),
                        ],
                        json!({"locked":locked}),
                    )?;
                }
                Ok(())
            })
            .await?;
            let runtime = &snapshot.block().runtime;
            let recorded = RecordingView::new(&snapshot);
            let intent = add(Amount::All)?;
            assert_eq!(
                intent
                    .build(&BuildCx {
                        view: &recorded,
                        origin: OriginView::Account(OWNER),
                        runtime,
                        clock: &SystemClock,
                        spec: AddCollateral::SPEC,
                    })
                    .await
                    .expect_err("no free alpha")
                    .to_string(),
                format!(
                    "nothing to lock as collateral: no free alpha on {} at netuid 513",
                    HOTKEY.to_ss58(2)
                )
            );
            assert_eq!(recorded.touches().len(), 2);
            assert!(
                recorded
                    .touches()
                    .iter()
                    .all(|touch| touch.item != "current_alpha_price")
            );
            client.close().await;
        }
        Ok(())
    }

    #[tokio::test]
    async fn collateral_floor_builds_without_reads_and_hotkey_defaults_match_python()
    -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture(|state| {
            state.seed_runtime("SwapRuntimeApi", "current_alpha_price", json!(20))?;
            Ok(())
        })
        .await?;
        let runtime = &snapshot.block().runtime;
        for netuid in [NetUid::ROOT, NETUID] {
            for rao in [0, 1, u64::MAX] {
                for prefix in [0, 2, 42] {
                    let requests = chain.requests().len();
                    let spelling = HOTKEY.to_ss58(prefix);
                    let intent = SetMinCollateral {
                        netuid,
                        min_alpha: DynBalance::from_rao(rao, netuid).into(),
                        hotkey_ss58: Some(spelling.parse()?),
                    };
                    let built = intent
                        .build(&BuildCx {
                            view: &snapshot,
                            origin: OriginView::Account(OWNER),
                            runtime,
                            clock: &SystemClock,
                            spec: SetMinCollateral::SPEC,
                        })
                        .await?;
                    let expected = expected_call(145, netuid, rao, None);
                    assert_eq!(built.call.as_bytes(), expected);
                    assert_eq!(CallBytes::decode(&expected, runtime)?, built.call);
                    assert!(built.extras.is_empty());
                    assert_eq!(intent.to_json()["hotkey_ss58"], spelling);
                    let default = SetMinCollateral {
                        hotkey_ss58: None,
                        ..intent
                    };
                    assert_eq!(
                        default
                            .build(&BuildCx {
                                view: &snapshot,
                                origin: OriginView::Keys(&PublicKeys),
                                runtime,
                                clock: &SystemClock,
                                spec: SetMinCollateral::SPEC,
                            })
                            .await?
                            .call,
                        built.call
                    );
                    assert!(
                        default
                            .build(&BuildCx {
                                view: &snapshot,
                                origin: OriginView::Account(OWNER),
                                runtime,
                                clock: &SystemClock,
                                spec: SetMinCollateral::SPEC,
                            })
                            .await
                            .is_err()
                    );
                    assert_eq!(chain.requests().len(), requests);
                }
            }
        }
        let mut intent = add(Money::from_rao(1).into())?;
        intent.hotkey_ss58 = None;
        assert!(
            intent
                .build(&BuildCx {
                    view: &snapshot,
                    origin: OriginView::Account(OWNER),
                    runtime,
                    clock: &SystemClock,
                    spec: AddCollateral::SPEC,
                })
                .await
                .is_err()
        );
        assert!(
            intent
                .build(&BuildCx {
                    view: &snapshot,
                    origin: OriginView::Keys(&PublicKeys),
                    runtime,
                    clock: &SystemClock,
                    spec: AddCollateral::SPEC,
                })
                .await
                .is_ok()
        );
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn collateral_preflight_keeps_unbounded_spend_shield_policy_and_all_warning()
    -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture(|state| {
            state.seed_runtime("SwapRuntimeApi", "current_alpha_price", json!(20))?;
            Ok(())
        })
        .await?;
        let runtime = &snapshot.block().runtime;
        let exact = add(Money::from_rao(1_000_000_000).into())?;
        let all = add(Amount::All)?;
        let floor = SetMinCollateral {
            netuid: NETUID,
            min_alpha: Money::from_rao(0).into(),
            hotkey_ss58: None,
        };
        let call = exact
            .build(&BuildCx {
                view: &snapshot,
                origin: OriginView::Keys(&PublicKeys),
                runtime,
                clock: &SystemClock,
                spec: AddCollateral::SPEC,
            })
            .await?
            .call;
        let requests = chain.requests().len();
        for (intent, summary, warnings, spend) in [
            (
                &exact as &dyn DynIntent,
                "lock 1.000000000α₅₁₃ as collateral on netuid 513 (fails if price moves >5.00%)",
                vec![],
                Spend::Unbounded,
            ),
            (
                &all as &dyn DynIntent,
                "lock ALL free alpha as collateral on netuid 513 (fails if price moves >5.00%)",
                vec!["locks every free (not already collateralized) alpha on this hotkey"],
                Spend::Unbounded,
            ),
            (
                &floor as &dyn DynIntent,
                "set collateral floor to 0.000000000α₅₁₃ on netuid 513",
                vec![],
                Spend::None,
            ),
        ] {
            let preview = intent
                .preflight_dyn(&PreflightCx {
                    view: &snapshot as &dyn DynView,
                    runtime,
                    clock: &SystemClock,
                    spec: intent.spec(),
                    client: &client,
                    dispatch_origin: OWNER,
                    fee_payer: OWNER,
                    call: &call,
                })
                .await?;
            assert_eq!(intent.summary_dyn(), summary);
            assert_eq!(preview.effects, [summary]);
            assert_eq!(preview.warnings, warnings);
            assert!(preview.blocks.is_empty());
            assert!(preview.required_free.is_none());
            assert!(preview.available_free.is_none());
            assert!(preview.estimated_fee.is_none());
            assert!(preview.facts.is_empty());
            assert_eq!(intent.spend_dyn(), spend);
            assert_eq!(intent.touches_netuids_dyn().as_slice(), [NETUID]);
            assert!(!intent.affects_all_subnets_dyn());
            assert_eq!(intent.spec().signer, "coldkey");
            assert_eq!(intent.spec().origin, "signed");
        }
        assert!(AddCollateral::SPEC.mev_shield_default);
        assert!(AddCollateral::SPEC.mev_shield_required);
        assert_eq!(AddCollateral::SPEC.all_amount_fields, ["amount_alpha"]);
        assert!(!SetMinCollateral::SPEC.mev_shield_required);
        assert!(SetMinCollateral::SPEC.all_amount_fields.is_empty());
        assert_eq!(chain.requests().len(), requests);
        client.close().await;
        Ok(())
    }

    #[test]
    fn collateral_tools_refuse_bad_native_fields_and_keep_python_defaults() -> anyhow::Result<()> {
        let json = crate::AnyIntent::from_tool(
            "add_collateral",
            json!({"netuid":513,"amount_alpha":" ALL "}),
        )?
        .to_json();
        assert_eq!(json["amount_alpha"], "all");
        assert_eq!(json["hotkey_ss58"], Value::Null);
        assert_eq!(json["rate_tolerance"], 0.05);
        for (op, amount_field) in [
            ("add_collateral", "amount_alpha"),
            ("set_min_collateral", "min_alpha"),
        ] {
            for invalid in [
                json!(-1),
                json!("18446744073.709551616"),
                json!(null),
                json!(true),
            ] {
                let mut args = json!({"netuid":513});
                args[amount_field] = invalid;
                assert!(crate::AnyIntent::from_tool(op, args).is_err());
            }
            for (field, value) in [
                ("netuid", json!(-1)),
                ("netuid", json!(65536)),
                ("hotkey_ss58", json!("invalid")),
                ("extra", json!(true)),
            ] {
                let mut args = json!({"netuid":513});
                args[amount_field] = json!("0");
                args[field] = value;
                assert!(crate::AnyIntent::from_tool(op, args).is_err());
            }
        }
        assert!(
            crate::AnyIntent::from_tool(
                "set_min_collateral",
                json!({"netuid":513,"min_alpha":"all"})
            )
            .is_err()
        );
        for tolerance in [json!(-0.1), json!(1.0), json!("NaN"), json!("inf")] {
            assert!(
                crate::AnyIntent::from_tool(
                    "add_collateral",
                    json!({"netuid":513,"amount_alpha":"all","rate_tolerance":tolerance})
                )
                .is_err()
            );
        }
        Ok(())
    }
}
