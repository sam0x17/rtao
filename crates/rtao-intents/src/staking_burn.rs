// Derived from RaoFoundation/subtensor sdk/python/bittensor/intents/{governance,staking}.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! TAO-funded subnet alpha buybacks with an explicit or spot-derived limit.

use rtao_client::{View, ViewExt};
use rtao_primitives::{Amount, Balance, NetUid, PriceRao, Spend, Tao, UnitError, pyfmt};
use rtao_runtime::{calls::subtensor_module, runtime_apis::swap_runtime_api};
use rtao_tx::{BuildCx, Built, Intent, IntentError};

pub use crate::StakeBurn;

// This intent belongs to the governance feature. Keep its small calculation
// independent of the staking feature's intent implementations.
fn exact_tao(amount: &Amount) -> Result<Balance<Tao>, IntentError> {
    match amount {
        Amount::Exact(amount) => Ok(amount.resolve(NetUid::ROOT)?.tao()?),
        Amount::All => Err(UnitError::AllNotAllowed.into()),
    }
}

fn check_tolerance(tolerance: f64) -> Result<(), IntentError> {
    if !(0.0..1.0).contains(&tolerance) {
        return Err(IntentError::validation(format!(
            "rate_tolerance must be a fraction in [0, 1), got {} (0.05 = 5%). To trade with no price bound, disable slippage protection (slippage_protection=False / --no-slippage-protection) instead.",
            pyfmt::float_repr(tolerance)
        )));
    }
    Ok(())
}

fn derived_limit(price: u64, tolerance: f64) -> Result<PriceRao, IntentError> {
    // Python converts the integer to binary64 before multiplying, then
    // truncates. Check the exclusive native limit before Rust's float cast,
    // which would otherwise saturate 2^64 to u64::MAX.
    let limit = (price as f64 * (1.0 + tolerance)).trunc();
    if limit >= 18_446_744_073_709_551_616.0 {
        return Err(UnitError::Overflow.into());
    }
    Ok(PriceRao(limit as u64))
}

impl Intent for StakeBurn {
    fn summary(&self) -> String {
        let amount = match &self.amount_tao {
            Amount::Exact(money) => money
                .resolve(NetUid::ROOT)
                .map(|amount| amount.to_string())
                .unwrap_or_else(|_| money.to_decimal_string()),
            // Invalid typed construction is rejected before execution.
            Amount::All => "all".into(),
        };
        format!("stake burn {amount} on netuid {}", self.netuid)
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        self.validate()?;
        let hotkey = cx.hotkey_address(self.hotkey_ss58.as_ref().map(|key| key.account()))?;
        let limit = if let Some(limit) = self.limit_price {
            PriceRao(limit)
        } else {
            let price = cx
                .view
                .call(&swap_runtime_api::CURRENT_ALPHA_PRICE, (self.netuid,))
                .await?;
            derived_limit(price, self.rate_tolerance.get())?
        };
        Ok(Built::new(
            subtensor_module::add_stake_burn(
                hotkey,
                self.netuid,
                exact_tao(&self.amount_tao)?.to_wire(),
                Some(limit),
            )
            .compose(cx.runtime)?,
        ))
    }

    fn validate(&self) -> Result<(), IntentError> {
        // Python checks the amount first and tolerance second, even when an
        // explicit limit makes the tolerance irrelevant to composition.
        exact_tao(&self.amount_tao)?;
        check_tolerance(self.rate_tolerance.get())
    }

    fn spend(&self) -> Spend {
        exact_tao(&self.amount_tao).map_or(Spend::Unbounded, Spend::Bounded)
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use rtao_client::{BlockRef, Client, ClientConfig, Network, Snapshot};
    use rtao_primitives::{AccountId, DynBalance, Ratio};
    use rtao_runtime::{CallBytes, RuntimeHandle};
    use rtao_signer::{KeyView, SignerError};
    use rtao_testkit::{FakeChain, FakeState, RecordingView, Reply, rpc::RpcClient};
    use rtao_tx::{FactsTitle, IntentInterface, OriginView, Policy, PreflightCx, SystemClock};
    use serde_json::{Value, json};

    const OWNER: AccountId = AccountId([7; 32]);
    const HOTKEY: AccountId = AccountId([8; 32]);
    const OUTER_PAYER: AccountId = AccountId([9; 32]);
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

    async fn fixture() -> anyhow::Result<(FakeChain, Client, Snapshot)> {
        let mut state = FakeState::try_default()?;
        state.seed_runtime(
            "SwapRuntimeApi",
            "current_alpha_price",
            json!(1_000_000_001_u64),
        )?;
        let chain = FakeChain::from_state(state);
        let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
        config.track_heads = false;
        config.runtime_cache_dir = None;
        config.config_store = None;
        let client = Client::from_rpc(RpcClient::new(chain.clone()), config).await?;
        let snapshot = client.at(BlockRef::Finalized).await?;
        Ok((chain, client, snapshot))
    }

    fn intent(rao: u64, limit: Option<u64>) -> anyhow::Result<StakeBurn> {
        Ok(StakeBurn {
            netuid: NETUID,
            amount_tao: Balance::<Tao>::from_rao(rao).into(),
            limit_price: limit,
            rate_tolerance: Ratio::new(0.05)?,
            hotkey_ss58: Some(HOTKEY.to_ss58(42).parse()?),
        })
    }

    fn expected_bytes(netuid: NetUid, amount: u64, limit: u64) -> Vec<u8> {
        // Pinned dispatches.rs declares call 132 and non-compact arguments:
        // AccountId32, NetUid(u16), TaoBalance(u64), Option<TaoBalance>.
        let mut expected = vec![7, 132];
        expected.extend_from_slice(&HOTKEY.0);
        expected.extend_from_slice(&netuid.0.to_le_bytes());
        expected.extend_from_slice(&amount.to_le_bytes());
        expected.push(1);
        expected.extend_from_slice(&limit.to_le_bytes());
        expected
    }

    fn decoded(call: &CallBytes, runtime: &RuntimeHandle) -> anyhow::Result<Value> {
        let value = runtime
            .core()
            .decode_spec(&rtao_core::runtime::TypeSpec::Call, call.as_bytes(), true)
            .map_err(rtao_core::CoreError::from)?;
        Ok(rtao_core::codec::value::to_corpus_json(&value))
    }

    #[tokio::test]
    async fn explicit_burn_limit_preserves_native_scale_and_defers_root_rejection()
    -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture().await?;
        let runtime = &snapshot.block().runtime;
        let requests = chain.requests().len();
        let recorded = RecordingView::new(&snapshot);
        for netuid in [NetUid::ROOT, NETUID, NetUid(u16::MAX)] {
            for rao in [0, 1, 0x0807_0605_0403_0201, u64::MAX] {
                for limit in [0, 1, u64::MAX] {
                    let mut intent = intent(rao, Some(limit))?;
                    intent.netuid = netuid;
                    let built = intent
                        .build(&BuildCx {
                            view: &recorded,
                            origin: OriginView::Account(OWNER),
                            runtime,
                            clock: &SystemClock,
                            spec: StakeBurn::SPEC,
                        })
                        .await?;
                    let expected = expected_bytes(netuid, rao, limit);
                    assert_eq!(built.call.as_bytes(), expected);
                    assert_eq!(CallBytes::decode(&expected, runtime)?, built.call);
                    assert!(built.extras.is_empty());
                    let call = decoded(&built.call, runtime)?;
                    assert_eq!(call["call_module"], "SubtensorModule");
                    assert_eq!(call["call_function"], "add_stake_burn");
                    assert_eq!(call["call_args"][0]["name"], "hotkey");
                    assert_eq!(call["call_args"][1]["name"], "netuid");
                    assert_eq!(call["call_args"][1]["value"], netuid.0);
                    assert_eq!(call["call_args"][2]["name"], "amount");
                    assert_eq!(call["call_args"][2]["value"], rao);
                    assert_eq!(call["call_args"][3]["name"], "limit");
                    assert_eq!(intent.spend(), Spend::Bounded(Balance::from_rao(rao)));
                }
            }
        }
        assert!(recorded.touches().is_empty());
        assert_eq!(chain.requests().len(), requests);
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn omitted_burn_limit_reads_spot_and_checks_python_float_rounding_before_encoding()
    -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture().await?;
        let runtime = &snapshot.block().runtime;
        let recorded = RecordingView::new(&snapshot);
        for (price, tolerance, expected_limit) in [
            (0_u64, 0.05, 0_u64),
            (19, 0.05, 19),
            (20, 0.05, 21),
            (1_000_000_001, 0.05, 1_050_000_001),
            (9_007_199_254_740_993, 0.0, 9_007_199_254_740_992),
            (9_007_199_254_740_995, 0.0, 9_007_199_254_740_996),
        ] {
            chain.mutate_state(|state| {
                state.seed_runtime_encoded(
                    "SwapRuntimeApi_current_alpha_price",
                    NETUID.0.to_le_bytes().to_vec(),
                    price.to_le_bytes().to_vec(),
                );
            });
            recorded.clear();
            let mut intent = intent(1, None)?;
            intent.rate_tolerance = Ratio::new(tolerance)?;
            let built = intent
                .build(&BuildCx {
                    view: &recorded,
                    origin: OriginView::Account(OWNER),
                    runtime,
                    clock: &SystemClock,
                    spec: StakeBurn::SPEC,
                })
                .await?;
            assert_eq!(
                built.call.as_bytes(),
                expected_bytes(NETUID, 1, expected_limit)
            );
            let touches = recorded.touches();
            assert_eq!(touches.len(), 1);
            assert_eq!(touches[0].item, "current_alpha_price");
            assert_eq!(recorded.block_hashes().len(), 1);
        }
        chain.mutate_state(|state| {
            state.seed_runtime_encoded(
                "SwapRuntimeApi_current_alpha_price",
                NETUID.0.to_le_bytes().to_vec(),
                u64::MAX.to_le_bytes().to_vec(),
            );
        });
        let mut overflowing = intent(1, None)?;
        overflowing.rate_tolerance = Ratio::new(0.0)?;
        assert!(matches!(
            overflowing
                .build(&BuildCx {
                    view: &snapshot,
                    origin: OriginView::Account(OWNER),
                    runtime,
                    clock: &SystemClock,
                    spec: StakeBurn::SPEC,
                })
                .await,
            Err(IntentError::UnitMismatch(UnitError::Overflow))
        ));
        chain.mutate_state(|state| {
            state.queue_reply("state_call", Reply::result(json!("0x")));
        });
        assert!(
            intent(1, None)?
                .build(&BuildCx {
                    view: &snapshot,
                    origin: OriginView::Account(OWNER),
                    runtime,
                    clock: &SystemClock,
                    spec: StakeBurn::SPEC,
                })
                .await
                .is_err()
        );
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn burn_hotkey_defaults_explicit_priority_and_ss58_spelling_match_python()
    -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture().await?;
        let runtime = &snapshot.block().runtime;
        let requests = chain.requests().len();
        let mut default = intent(1, Some(2))?;
        default.hotkey_ss58 = None;
        let built = default
            .build(&BuildCx {
                view: &snapshot,
                origin: OriginView::Keys(&PublicKeys),
                runtime,
                clock: &SystemClock,
                spec: StakeBurn::SPEC,
            })
            .await?;
        assert_eq!(built.call.as_bytes(), expected_bytes(NETUID, 1, 2));
        for prefix in [0, 2, 42] {
            let spelling = HOTKEY.to_ss58(prefix);
            let mut explicit = default.clone();
            explicit.hotkey_ss58 = Some(spelling.parse()?);
            assert_eq!(explicit.to_json()["hotkey_ss58"], spelling);
            assert_eq!(
                explicit
                    .build(&BuildCx {
                        view: &snapshot,
                        origin: OriginView::Account(OWNER),
                        runtime,
                        clock: &SystemClock,
                        spec: StakeBurn::SPEC,
                    })
                    .await?
                    .call,
                built.call
            );
        }
        assert_eq!(chain.requests().len(), requests);
        // Resolving the hotkey precedes the derived-price read.
        default.limit_price = None;
        assert!(
            default
                .build(&BuildCx {
                    view: &snapshot,
                    origin: OriginView::Account(OWNER),
                    runtime,
                    clock: &SystemClock,
                    spec: StakeBurn::SPEC,
                })
                .await
                .is_err()
        );
        assert_eq!(chain.requests().len(), requests);
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn burn_preflight_inherits_default_fees_warnings_and_exact_spend_policy()
    -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture().await?;
        let runtime = &snapshot.block().runtime;
        let intent = intent(1_000_000_001, Some(0))?;
        let built = intent
            .build(&BuildCx {
                view: &snapshot,
                origin: OriginView::Account(OWNER),
                runtime,
                clock: &SystemClock,
                spec: StakeBurn::SPEC,
            })
            .await?;
        let requests = chain.requests().len();
        let recorded = RecordingView::new(&snapshot);
        let preview = intent
            .preflight(&PreflightCx {
                view: &recorded,
                runtime,
                clock: &SystemClock,
                spec: StakeBurn::SPEC,
                client: &client,
                dispatch_origin: OWNER,
                fee_payer: OUTER_PAYER,
                call: &built.call,
            })
            .await?;
        assert_eq!(intent.summary(), "stake burn τ1.000000001 on netuid 513");
        assert_eq!(preview.effects, [intent.summary()]);
        assert!(preview.warnings.is_empty());
        assert!(preview.blocks.is_empty());
        assert!(preview.required_free.is_none());
        assert!(preview.available_free.is_none());
        assert!(preview.estimated_fee.is_none());
        assert!(preview.facts.is_empty());
        assert_eq!(preview.facts_title, FactsTitle::Fees);
        assert_eq!(intent.touches_netuids().as_slice(), [NETUID]);
        assert!(!intent.affects_all_subnets());
        assert_eq!(StakeBurn::SPEC.signer, "coldkey");
        assert_eq!(StakeBurn::SPEC.origin, "signed");
        assert!(StakeBurn::SPEC.mev_shield_default);
        assert!(!StakeBurn::SPEC.mev_shield_required);
        assert!(StakeBurn::SPEC.all_amount_fields.is_empty());
        assert!(StakeBurn::SPEC.verify.is_none());
        let policy = Policy {
            max_spend: Some(Balance::from_rao(1_000_000_000)),
            allowed_netuids: Some([NetUid(1)].into()),
            ..Default::default()
        };
        assert_eq!(
            policy.check(&intent, None),
            [
                "spend τ1.000000001 exceeds max_spend_tao τ1.000000000",
                "netuid 513 not in allowed_netuids [1]",
            ]
        );
        let policy = Policy {
            max_spend: Some(Balance::from_rao(1_000_000_001)),
            allowed_netuids: Some([NETUID].into()),
            ..Default::default()
        };
        assert!(policy.check(&intent, None).is_empty());
        assert!(recorded.touches().is_empty());
        assert_eq!(chain.requests().len(), requests);
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        client.close().await;
        Ok(())
    }

    #[test]
    fn burn_constructor_keeps_exact_tao_boundaries_and_checks_amount_before_tolerance()
    -> anyhow::Result<()> {
        for rao in [0, 1, 9_007_199_254_740_993, u64::MAX] {
            let intent = intent(rao, Some(u64::MAX))?;
            intent.validate()?;
            assert_eq!(exact_tao(&intent.amount_tao)?.rao(), rao);
            assert_eq!(intent.spend(), Spend::Bounded(Balance::from_rao(rao)));
            let json = intent.to_json();
            assert_eq!(
                crate::AnyIntent::from_tool("stake_burn", json.clone())?.to_json(),
                json
            );
        }
        let mut wrong_unit = intent(1, Some(1))?;
        wrong_unit.amount_tao = DynBalance::from_rao(1, NETUID).into();
        wrong_unit.rate_tolerance = Ratio::new(1.0)?;
        assert!(matches!(
            wrong_unit.validate(),
            Err(IntentError::UnitMismatch(UnitError::UnitMismatch {
                expected: NetUid::ROOT,
                actual: NETUID,
            }))
        ));
        wrong_unit.amount_tao = Amount::All;
        assert!(matches!(
            wrong_unit.validate(),
            Err(IntentError::UnitMismatch(UnitError::AllNotAllowed))
        ));
        assert_eq!(wrong_unit.spend(), Spend::Unbounded);
        for tolerance in [-0.1, 1.0, 1.5] {
            let mut invalid = intent(1, Some(0))?;
            invalid.rate_tolerance = Ratio::new(tolerance)?;
            assert_eq!(
                invalid
                    .validate()
                    .expect_err("invalid tolerance")
                    .to_string(),
                format!(
                    "rate_tolerance must be a fraction in [0, 1), got {} (0.05 = 5%). To trade with no price bound, disable slippage protection (slippage_protection=False / --no-slippage-protection) instead.",
                    pyfmt::float_repr(tolerance)
                )
            );
        }
        for tolerance in [-0.0, 0.0, 0.999_999_999_999_999_9] {
            let mut valid = intent(1, Some(0))?;
            valid.rate_tolerance = Ratio::new(tolerance)?;
            valid.validate()?;
        }
        Ok(())
    }

    #[test]
    fn burn_tools_normalize_defaults_and_refuse_all_and_invalid_native_fields() -> anyhow::Result<()>
    {
        assert_eq!(
            crate::AnyIntent::from_tool("stake_burn", json!({"netuid":1,"amount_tao":1}))?
                .to_json(),
            json!({
                "op":"stake_burn","netuid":1,"amount_tao":"1",
                "limit_price":null,"rate_tolerance":0.05,"hotkey_ss58":null,
            })
        );
        let valid =
            json!({"netuid":513,"amount_tao":"18446744073.709551615","limit_price":u64::MAX});
        assert!(crate::AnyIntent::from_tool("stake_burn", valid.clone()).is_ok());
        for (field, value) in [
            ("netuid", json!(-1)),
            ("netuid", json!(65536)),
            ("amount_tao", json!("all")),
            ("amount_tao", json!(" ALL ")),
            ("amount_tao", json!(-1)),
            ("amount_tao", json!("18446744073.709551616")),
            ("amount_tao", Value::Null),
            ("amount_tao", json!(true)),
            ("limit_price", json!(-1)),
            ("limit_price", serde_json::from_str("18446744073709551616")?),
            ("rate_tolerance", json!(-0.1)),
            ("rate_tolerance", json!(1)),
            ("rate_tolerance", json!("nan")),
            ("rate_tolerance", json!("inf")),
            ("hotkey_ss58", json!("invalid")),
            ("hotkey_ss58", json!("")),
            ("extra", json!(true)),
        ] {
            let mut invalid = valid.clone();
            invalid[field] = value;
            assert!(
                crate::AnyIntent::from_tool("stake_burn", invalid).is_err(),
                "{field}"
            );
        }
        for field in ["netuid", "amount_tao"] {
            let mut invalid = valid.clone();
            invalid
                .as_object_mut()
                .expect("fixture object")
                .remove(field);
            assert!(crate::AnyIntent::from_tool("stake_burn", invalid).is_err());
        }
        for invalid in [json!([]), Value::Null] {
            assert!(crate::AnyIntent::from_tool("stake_burn", invalid).is_err());
        }
        Ok(())
    }
}
