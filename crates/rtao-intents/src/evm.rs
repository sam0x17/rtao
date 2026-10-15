// Derived from RaoFoundation/subtensor sdk/python/bittensor/intents/evm.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/evm/addresses.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Native TAO funding and withdrawal across the EVM account mirror.

use rtao_client::View;
use rtao_core::{DynValue, codec::Value};
use rtao_primitives::{AccountId, Amount, NetUid, ResolvedAmount, Spend};
use rtao_runtime::calls::{balances, evm};
use rtao_tx::{BuildCx, Built, Intent, IntentError, PreflightCx};

pub use crate::{EvmWithdraw, FundEvmKey};

// Keep this constructor hook before generated Amount deserialization: Python
// normalizes the address before its money field. Missing/unknown dataclass
// arguments still fail before __post_init__, so leave those to the parser.
pub(crate) fn canonicalize_fund_input(args: &mut serde_json::Value) -> Result<(), IntentError> {
    let Some(fields) = args.as_object_mut() else {
        return Ok(());
    };
    if fields.len() != 2 || !fields.contains_key("amount_tao") {
        return Ok(());
    }
    let Some(address) = fields
        .get("evm_address")
        .and_then(serde_json::Value::as_str)
    else {
        return Ok(());
    };
    let (address, _) = normalize_h160(address)?;
    fields.insert("evm_address".into(), serde_json::Value::String(address));
    Ok(())
}

/// Preserve constructor normalization for programmatically built intent values.
pub(crate) fn fund_json(intent: &FundEvmKey) -> serde_json::Value {
    let mut value = crate::args_with_op("fund_evm_key", intent);
    if let Ok((address, _)) = normalize_h160(&intent.evm_address) {
        value["evm_address"] = serde_json::Value::String(address);
    }
    value
}

fn python_whitespace(character: char) -> bool {
    character.is_whitespace() || matches!(character, '\u{1c}'..='\u{1f}')
}

// bytes.fromhex permits ASCII whitespace between complete byte pairs. The
// upstream is_h160 checks the text length, not the decoded length, so retain
// its accepted interior-whitespace inputs instead of adding a second policy.
fn normalize_h160(input: &str) -> Result<(String, Vec<u8>), IntentError> {
    let invalid = || {
        IntentError::validation(format!(
            "not a valid EVM (h160) address: {}",
            rtao_primitives::pyfmt::string_repr(input)
        ))
    };
    let text = input.trim_matches(python_whitespace);
    let body = text.strip_prefix("0x").unwrap_or(text);
    if body.chars().count() != 40 {
        return Err(invalid());
    }
    let mut bytes = Vec::with_capacity(20);
    for group in body
        .as_bytes()
        .split(|byte| matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c))
    {
        bytes.extend(hex::decode(group).map_err(|_| invalid())?);
    }
    Ok((format!("0x{}", body.to_ascii_lowercase()), bytes))
}

fn mirror_account(address: &str) -> Result<AccountId, IntentError> {
    let (_, bytes) = normalize_h160(address)?;
    let mut payload = Vec::with_capacity(4 + bytes.len());
    payload.extend_from_slice(b"evm:");
    payload.extend_from_slice(&bytes);
    let hash = rtao_core::codec::storage::hash_param("Blake2_256", &payload)?;
    let hash = hash.try_into().map_err(|_| {
        IntentError::validation("Blake2_256 returned an EVM mirror hash with an invalid length")
    })?;
    Ok(AccountId(hash))
}

fn amount_summary(amount: &Amount) -> String {
    match amount.resolve(NetUid::ROOT, true) {
        Ok(ResolvedAmount::All) => "ALL TAO".into(),
        Ok(ResolvedAmount::Exact(amount)) => amount.to_string(),
        // A typed caller can supply the wrong currency; validate/build refuse
        // it before execution while this infallible presentation hook remains usable.
        Err(_) => match amount {
            Amount::All => "ALL TAO".into(),
            Amount::Exact(amount) => amount.to_decimal_string(),
        },
    }
}

impl FundEvmKey {
    /// Native SS58 account whose balance backs the normalized EVM address.
    #[doc(alias = "bittensor.intents.evm.FundEvmKey.mirror_ss58")]
    pub fn mirror_ss58(&self) -> Result<String, IntentError> {
        Ok(mirror_account(&self.evm_address)?.to_string())
    }
}

impl Intent for FundEvmKey {
    fn summary(&self) -> String {
        let address = normalize_h160(&self.evm_address)
            .map(|(address, _)| address)
            .unwrap_or_else(|_| self.evm_address.clone());
        let mirror = self.mirror_ss58().unwrap_or_default();
        format!(
            "fund EVM address {address} (mirror {mirror}) with {}",
            amount_summary(&self.amount_tao)
        )
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        let mirror = mirror_account(&self.evm_address)?;
        let destination = DynValue::new(Value::Bytes(mirror.0.to_vec()));
        let call = match self.amount_tao.resolve(NetUid::ROOT, true)? {
            ResolvedAmount::All => balances::transfer_all(destination, true),
            ResolvedAmount::Exact(amount) => {
                balances::transfer_keep_alive(destination, amount.tao()?.to_wire())
            }
        };
        Ok(Built::new(call.compose(cx.runtime)?))
    }

    async fn warnings<V: View + ?Sized>(
        &self,
        _cx: &PreflightCx<'_, V>,
    ) -> Result<Vec<String>, IntentError> {
        Ok(vec![
            "only the EVM private key for this address can move the funds afterwards".into(),
        ])
    }

    fn spend(&self) -> Spend {
        match self.amount_tao.resolve(NetUid::ROOT, true) {
            Ok(ResolvedAmount::Exact(amount)) => {
                amount.tao().map_or(Spend::Unbounded, Spend::Bounded)
            }
            Ok(ResolvedAmount::All) | Err(_) => Spend::Unbounded,
        }
    }

    fn validate(&self) -> Result<(), IntentError> {
        normalize_h160(&self.evm_address)?;
        self.amount_tao.resolve(NetUid::ROOT, true)?;
        Ok(())
    }
}

impl Intent for EvmWithdraw {
    fn summary(&self) -> String {
        format!(
            "claim {} from the coldkey's EVM mirror",
            amount_summary(&self.amount_tao)
        )
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        let resolved = self.amount_tao.resolve(NetUid::ROOT, true)?;
        let coldkey = cx.coldkey_address()?;
        let address = format!("0x{}", hex::encode(&coldkey.as_bytes()[..20]));
        let amount = match resolved {
            ResolvedAmount::All => {
                let mirror = mirror_account(&address)?;
                let amount = rtao_tx::multisig::free_balance(cx.view, mirror).await?;
                if amount.is_zero() {
                    return Err(IntentError::validation(
                        "nothing to claim: the EVM deposit address is empty",
                    ));
                }
                amount
            }
            ResolvedAmount::Exact(amount) => amount.tao()?,
        };
        let call = evm::withdraw(DynValue::new(Value::Str(address)), amount.to_wire());
        Ok(Built::new(call.compose(cx.runtime)?))
    }

    fn validate(&self) -> Result<(), IntentError> {
        self.amount_tao.resolve(NetUid::ROOT, true)?;
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use rtao_client::{BlockRef, Client, ClientConfig, DynView, Network, Snapshot};
    use rtao_primitives::{Alpha, Balance, Tao};
    use rtao_runtime::{CallBytes, RuntimeHandle};
    use rtao_testkit::{FakeChain, FakeState, RecordingView, rpc::RpcClient};
    use rtao_tx::{DynIntent, IntentInterface, OriginView, SystemClock};
    use serde_json::json;

    const COLDKEY: AccountId = AccountId([0x11; 32]);
    const OTHER: AccountId = AccountId([0xab; 32]);
    const ADDRESS: &str = "0x1111111111111111111111111111111111111111";
    // Pinned Python tests/unit/test_evm.py::H160_TO_SS58_VECTORS.
    const MIRROR: &str = "5DDYKUgyHe2Sx8a9oTZPAXrgFaTiGnYZXrecNdeC9bG7TbtX";
    const MIRROR_KEY: &str = "32f955c958e51189a4921aed41ef00818f7368dfaec8d9969f091006f8066228";
    const OTHER_MIRROR: &str = "5F2S3KWCNrTLuuwPbA6b3kSBveGS27yVvQVipym8TwcvP2cs";

    async fn fixture(free: u64) -> anyhow::Result<(FakeChain, Client, Snapshot)> {
        let mut state = FakeState::try_default()?;
        for (account, free) in [
            (MIRROR.to_owned(), free),
            (OTHER_MIRROR.to_owned(), 7),
            (COLDKEY.to_string(), 999),
        ] {
            state.seed_python(
                "System",
                "Account",
                &[json!(account)],
                json!({"data":{"free":free}}),
            )?;
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

    fn decode(call: &CallBytes, runtime: &RuntimeHandle) -> anyhow::Result<serde_json::Value> {
        let decoded = runtime
            .core()
            .decode_spec(&rtao_core::runtime::TypeSpec::Call, call.as_bytes(), true)
            .map_err(rtao_core::CoreError::from)?;
        Ok(rtao_core::codec::value::to_corpus_json(&decoded))
    }

    #[test]
    fn evm_funding_uses_pinned_python_mirror_vectors_and_normalizes_at_construction()
    -> anyhow::Result<()> {
        for (address, mirror) in [
            (ADDRESS, MIRROR),
            ("0xabababababababababababababababababababab", OTHER_MIRROR),
            (
                "0x1234567890123456789012345678901234567890",
                "5Ettm6fSye3WnsW22Z1UNYa7Mo4gm2KQdcVYHs6rYuJHnhtj",
            ),
        ] {
            assert_eq!(mirror_account(address)?.to_string(), mirror);
            assert_eq!(
                FundEvmKey {
                    evm_address: address.into(),
                    amount_tao: Amount::All,
                }
                .mirror_ss58()?,
                mirror
            );
        }
        assert_eq!(hex::encode(mirror_account(ADDRESS)?.0), MIRROR_KEY);
        let input = "\u{1c}\u{a0} ABABABABABABABABABABABABABABABABABABABAB \u{1f}";
        let intent = crate::AnyIntent::from_tool(
            "fund_evm_key",
            json!({"evm_address":input,"amount_tao":"1.25"}),
        )?;
        assert_eq!(
            intent.to_json()["evm_address"],
            "0xabababababababababababababababababababab"
        );
        let typed = FundEvmKey {
            evm_address: input.into(),
            amount_tao: Amount::parse("1.25")?,
        };
        assert_eq!(typed.to_json(), intent.to_json());
        assert_eq!(
            intent.as_dyn().summary_dyn(),
            format!(
                "fund EVM address 0xabababababababababababababababababababab (mirror {OTHER_MIRROR}) with τ1.250000000"
            )
        );
        assert_eq!(
            intent.as_dyn().spend_dyn(),
            Spend::Bounded(Balance::<Tao>::from_rao(1_250_000_000))
        );
        Ok(())
    }

    #[test]
    fn evm_funding_preserves_address_validation_and_constructor_error_order() -> anyhow::Result<()>
    {
        for address in [
            "",
            "0x",
            "0X1111111111111111111111111111111111111111",
            "0x111111111111111111111111111111111111111",
            "0xgg11111111111111111111111111111111111111",
            "0x1 11111111111111111111111111111111111111",
            "0xé111111111111111111111111111111111111111",
        ] {
            let expected = format!(
                "not a valid EVM (h160) address: {}",
                rtao_primitives::pyfmt::string_repr(address)
            );
            assert_eq!(
                normalize_h160(address)
                    .expect_err("bad address must refuse")
                    .to_string(),
                expected
            );
            assert_eq!(
                crate::AnyIntent::from_tool(
                    "fund_evm_key",
                    json!({"evm_address":address,"amount_tao":"not an amount"})
                )
                .expect_err("address normalization runs before money")
                .to_string(),
                expected
            );
        }
        let address = format!("0x11  {}", "22".repeat(18));
        let (normalized, decoded) = normalize_h160(&address)?;
        assert_eq!(normalized, address);
        assert_eq!(decoded.len(), 19);
        assert_eq!(decoded[0], 0x11);
        assert!(decoded[1..].iter().all(|byte| *byte == 0x22));
        for input in [
            json!({"evm_address":"bad"}),
            json!({"amount_tao":"1"}),
            json!({"evm_address":"bad","amount_tao":"1","unexpected":true}),
        ] {
            assert!(crate::AnyIntent::from_tool("fund_evm_key", input).is_err());
        }
        assert!(
            crate::AnyIntent::from_tool(
                "fund_evm_key",
                json!({"evm_address":"bad","amount_tao":"1","unexpected":true}),
            )
            .expect_err("unknown constructor argument precedes post-init normalization")
            .to_string()
            .contains("unknown field `unexpected`")
        );
        Ok(())
    }

    #[tokio::test]
    async fn evm_funding_keeps_alive_and_encodes_native_compact_amounts_or_all()
    -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture(555).await?;
        let runtime = &snapshot.block().runtime;
        let recorded = RecordingView::new(snapshot.clone());
        let requests = chain.requests().len();
        for (rao, compact) in [
            (0, vec![0]),
            (1, vec![4]),
            (63, vec![252]),
            (64, vec![1, 1]),
            (u64::MAX, vec![19, 255, 255, 255, 255, 255, 255, 255, 255]),
        ] {
            let intent = FundEvmKey {
                evm_address: ADDRESS.into(),
                amount_tao: Balance::<Tao>::from_rao(rao).into(),
            };
            let built = intent
                .build(&BuildCx {
                    view: &recorded,
                    origin: OriginView::Account(COLDKEY),
                    runtime,
                    clock: &SystemClock,
                    spec: FundEvmKey::SPEC,
                })
                .await?;
            let mut expected = vec![5, 3, 0];
            expected.extend_from_slice(&hex::decode(MIRROR_KEY)?);
            expected.extend_from_slice(&compact);
            assert_eq!(built.call.as_bytes(), expected);
            assert_eq!(CallBytes::decode(&expected, runtime)?, built.call);
            assert!(built.extras.is_empty());
            let decoded = decode(&built.call, runtime)?;
            assert_eq!(decoded["call_module"], "Balances");
            assert_eq!(decoded["call_function"], "transfer_keep_alive");
            assert_eq!(decoded["call_args"][1]["value"], rao);
            assert_eq!(
                intent.spend(),
                Spend::Bounded(Balance::<Tao>::from_rao(rao))
            );
        }
        let intent = FundEvmKey {
            evm_address: ADDRESS.into(),
            amount_tao: Amount::All,
        };
        let built = intent
            .build(&BuildCx {
                view: &recorded,
                origin: OriginView::Account(COLDKEY),
                runtime,
                clock: &SystemClock,
                spec: FundEvmKey::SPEC,
            })
            .await?;
        let mut expected = vec![5, 4, 0];
        expected.extend_from_slice(&hex::decode(MIRROR_KEY)?);
        expected.push(1);
        assert_eq!(built.call.as_bytes(), expected);
        assert_eq!(CallBytes::decode(&expected, runtime)?, built.call);
        assert_eq!(decode(&built.call, runtime)?["call_args"][1]["value"], true);
        assert_eq!(intent.spend(), Spend::Unbounded);
        assert_eq!(
            intent.summary(),
            format!("fund EVM address {ADDRESS} (mirror {MIRROR}) with ALL TAO")
        );
        assert!(recorded.touches().is_empty());
        assert_eq!(chain.requests().len(), requests);
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn evm_withdraw_exact_never_reads_and_all_uses_effective_coldkey_mirror_free_balance()
    -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture(u64::MAX).await?;
        let runtime = &snapshot.block().runtime;
        let recorded = RecordingView::new(snapshot.clone());
        for rao in [0, 1, u64::MAX] {
            let intent = EvmWithdraw {
                amount_tao: Balance::<Tao>::from_rao(rao).into(),
            };
            let built = intent
                .build(&BuildCx {
                    view: &recorded,
                    origin: OriginView::Account(COLDKEY),
                    runtime,
                    clock: &SystemClock,
                    spec: EvmWithdraw::SPEC,
                })
                .await?;
            let mut expected = vec![22, 0];
            expected.extend_from_slice(&[0x11; 20]);
            expected.extend_from_slice(&rao.to_le_bytes());
            assert_eq!(built.call.as_bytes(), expected);
            assert_eq!(CallBytes::decode(&expected, runtime)?, built.call);
            assert!(built.extras.is_empty());
            let decoded = decode(&built.call, runtime)?;
            assert_eq!(decoded["call_module"], "EVM");
            assert_eq!(decoded["call_function"], "withdraw");
            assert_eq!(decoded["call_args"][0]["value"], ADDRESS);
            assert_eq!(decoded["call_args"][1]["value"], rao);
            assert_eq!(intent.spend(), Spend::None);
        }
        assert!(recorded.touches().is_empty());
        let all = EvmWithdraw {
            amount_tao: Amount::All,
        };
        for (origin, byte, free) in [(COLDKEY, 0x11, u64::MAX), (OTHER, 0xab, 7)] {
            recorded.clear();
            let built = all
                .build(&BuildCx {
                    view: &recorded,
                    origin: OriginView::Account(origin),
                    runtime,
                    clock: &SystemClock,
                    spec: EvmWithdraw::SPEC,
                })
                .await?;
            let mut expected = vec![22, 0];
            expected.extend_from_slice(&[byte; 20]);
            expected.extend_from_slice(&free.to_le_bytes());
            assert_eq!(built.call.as_bytes(), expected);
            assert_eq!(CallBytes::decode(&expected, runtime)?, built.call);
            let touches = recorded.touches();
            assert_eq!(touches.len(), 1);
            assert_eq!(touches[0].pallet_or_api, "System");
            assert_eq!(touches[0].item, "Account");
            assert_eq!(recorded.block_hashes().len(), 1);
        }
        assert_eq!(all.summary(), "claim ALL TAO from the coldkey's EVM mirror");
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn evm_withdraw_all_refuses_empty_mirrors_without_spending_origin_balance()
    -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture(0).await?;
        let intent = EvmWithdraw {
            amount_tao: Amount::All,
        };
        for origin in [COLDKEY, AccountId::ZERO] {
            let error = intent
                .build(&BuildCx {
                    view: &snapshot,
                    origin: OriginView::Account(origin),
                    runtime: &snapshot.block().runtime,
                    clock: &SystemClock,
                    spec: EvmWithdraw::SPEC,
                })
                .await
                .expect_err("empty mirror must refuse");
            assert_eq!(
                error.to_string(),
                "nothing to claim: the EVM deposit address is empty"
            );
        }
        assert_eq!(intent.spend(), Spend::None);
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn evm_preflights_keep_exact_warnings_and_inherited_policy() -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture(55).await?;
        let runtime = &snapshot.block().runtime;
        let requests = chain.requests().len();
        let fund = FundEvmKey {
            evm_address: ADDRESS.into(),
            amount_tao: Amount::All,
        };
        let withdraw = EvmWithdraw {
            amount_tao: Balance::<Tao>::from_rao(1).into(),
        };
        for (intent, warnings) in [
            (
                &fund as &dyn DynIntent,
                vec!["only the EVM private key for this address can move the funds afterwards"],
            ),
            (&withdraw as &dyn DynIntent, vec![]),
        ] {
            let built = intent
                .build_dyn(&BuildCx {
                    view: &snapshot as &dyn DynView,
                    origin: OriginView::Account(COLDKEY),
                    runtime,
                    clock: &SystemClock,
                    spec: intent.spec(),
                })
                .await?;
            let preview = intent
                .preflight_dyn(&PreflightCx {
                    view: &snapshot as &dyn DynView,
                    runtime,
                    clock: &SystemClock,
                    spec: intent.spec(),
                    client: &client,
                    dispatch_origin: COLDKEY,
                    fee_payer: OTHER,
                    call: &built.call,
                })
                .await?;
            assert_eq!(preview.effects, vec![intent.summary_dyn()]);
            assert_eq!(preview.warnings, warnings);
            assert!(preview.blocks.is_empty());
            assert!(preview.facts.is_empty());
            assert!(preview.required_free.is_none());
            assert!(preview.available_free.is_none());
            assert!(preview.estimated_fee.is_none());
            assert!(intent.touches_netuids_dyn().is_empty());
            assert!(!intent.affects_all_subnets_dyn());
            assert_eq!(intent.spec().signer, "coldkey");
            assert_eq!(intent.spec().origin, "signed");
            assert!(intent.spec().all_amount_fields.contains(&"amount_tao"));
            assert_eq!(
                crate::AnyIntent::from_tool(intent.spec().name, intent.to_json_dyn())?.to_json(),
                intent.to_json_dyn()
            );
        }
        assert_eq!(
            withdraw.summary(),
            "claim τ0.000000001 from the coldkey's EVM mirror"
        );
        assert_eq!(chain.requests().len(), requests);
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        client.close().await;
        Ok(())
    }

    #[test]
    fn evm_amounts_reject_wrong_currency_negative_and_out_of_native_range() -> anyhow::Result<()> {
        let alpha = Balance::<Alpha>::new(NetUid(1), 1)?;
        let fund = FundEvmKey {
            evm_address: ADDRESS.into(),
            amount_tao: alpha.into(),
        };
        let withdraw = EvmWithdraw {
            amount_tao: alpha.into(),
        };
        assert!(fund.validate().is_err());
        assert!(withdraw.validate().is_err());
        assert_eq!(fund.spend(), Spend::Unbounded);
        for amount in [
            json!("-0.000000001"),
            json!("18446744073.709551616"),
            json!("NaN"),
            json!("Infinity"),
            json!(null),
        ] {
            assert!(
                crate::AnyIntent::from_tool(
                    "fund_evm_key",
                    json!({"evm_address":ADDRESS,"amount_tao":amount})
                )
                .is_err()
            );
            assert!(
                crate::AnyIntent::from_tool("evm_withdraw", json!({"amount_tao":amount})).is_err()
            );
        }
        for amount in [json!(0), json!("18446744073.709551615"), json!(" aLl ")] {
            assert!(
                crate::AnyIntent::from_tool(
                    "fund_evm_key",
                    json!({"evm_address":ADDRESS,"amount_tao":amount})
                )
                .is_ok()
            );
            assert!(
                crate::AnyIntent::from_tool("evm_withdraw", json!({"amount_tao":amount})).is_ok()
            );
        }
        assert!(crate::AnyIntent::from_tool("evm_withdraw", json!({})).is_err());
        assert!(
            crate::AnyIntent::from_tool("evm_withdraw", json!({"amount_tao":1,"unexpected":true}))
                .is_err()
        );
        Ok(())
    }
}
