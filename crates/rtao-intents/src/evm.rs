// Derived from RaoFoundation/subtensor sdk/python/bittensor/intents/evm.py @ b6a81d144078eac18bd25c3af7cb31579bb670f3 (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/evm/addresses.py @ b6a81d144078eac18bd25c3af7cb31579bb670f3 (Apache-2.0); modified by rtao.
//! Native TAO funding and withdrawal across the EVM account mirror.
//!
//! Registered hashed accounts bind their truncated EVM alias to the complete
//! native account (`HashedAccounts.EvmAliases`); [`resolve_evm_recipient`]
//! reads the active mapping, while [`FundEvmKey::mirror_ss58`] stays the pure
//! legacy mirror.

use rtao_client::{View, ViewExt};
use rtao_core::{DynValue, codec::Value};
use rtao_primitives::{
    AccountId, Amount, NetUid, ResolvedAmount, Spend,
    receiving::{Recipient, descriptor_account, descriptor_bytes},
};
use rtao_runtime::{
    RuntimeHandle,
    calls::{balances, evm},
    storage::hashed_accounts as st,
};
use rtao_tx::{
    BuildCx, Built, Intent, IntentError, PreflightCx,
    hashed::{ProtectedKind, hashed_accounts_enabled, protected_account_kind},
};

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

fn normalize_h160(input: &str) -> Result<(String, Vec<u8>), IntentError> {
    let address = rtao_primitives::evm::normalize_h160(input)
        .map_err(|error| IntentError::validation(error.to_string()))?;
    let bytes = rtao_primitives::evm::decode_h160(input)
        .map_err(|error| IntentError::validation(error.to_string()))?;
    Ok((address, bytes))
}

fn mirror_account(address: &str) -> Result<AccountId, IntentError> {
    rtao_primitives::evm::mirror_account(address).map_err(|error| match error {
        rtao_primitives::evm::AddressError::Core(error) => IntentError::from(error),
        error => IntentError::validation(error.to_string()),
    })
}

fn has_constant(runtime: &RuntimeHandle, pallet: &str, name: &str) -> bool {
    runtime.core().constant(pallet, name).is_some()
}

/// Whether the runtime credits EVM addresses natively (`EVM.deposit`, which
/// resolves the active alias mapping during execution).
async fn native_deposit_supported<V: View + ?Sized>(
    view: &V,
    runtime: &RuntimeHandle,
) -> Result<bool, IntentError> {
    if !has_constant(runtime, "EVM", "NativeDepositSupported") {
        return Ok(false);
    }
    Ok(view
        .constant(&rtao_runtime::constants::evm::NATIVE_DEPOSIT_SUPPORTED)
        .await?)
}

fn legacy(account: AccountId) -> Recipient {
    Recipient {
        address: account.to_string(),
        account,
        descriptor: None,
    }
}

fn invalid_alias(reason: &str) -> IntentError {
    IntentError::validation(format!("invalid protected EVM alias binding: {reason}"))
}

/// Python `evm.addresses.resolve_evm_recipient`: the account an EVM
/// address's balance lives at, with the receiving address of a registered
/// hashed alias.
///
/// Chains without hashed accounts, and unbound addresses, keep the legacy
/// mirror. A bound alias must name an account whose first twenty bytes are
/// the address and which is registered with a descriptor naming that account;
/// a derived authority binding is refused unless the caller has checked it
/// is final (Python reads finalized state, which a pinned view cannot), so it
/// fails closed here. Malformed bindings always fail closed.
#[doc(alias = "bittensor.evm.addresses.resolve_evm_recipient")]
pub async fn resolve_evm_recipient<V: View + ?Sized>(
    view: &V,
    runtime: &RuntimeHandle,
    evm_address: &str,
) -> Result<Recipient, IntentError> {
    let (_, bytes) = normalize_h160(evm_address)?;
    let legacy_account = mirror_account(evm_address)?;
    if !has_constant(runtime, "HashedAccounts", "RegistrationDeposit")
        || runtime
            .storage_entry("HashedAccounts", "EvmAliases")
            .is_none()
    {
        return Ok(legacy(legacy_account));
    }
    let address: [u8; 20] = bytes
        .as_slice()
        .try_into()
        .map_err(|_| IntentError::validation("an EVM alias is exactly 20 bytes"))?;
    let Some(bound) = view.storage(&st::EVM_ALIASES, (address,)).await? else {
        return Ok(legacy(legacy_account));
    };
    if bound.0[..20] != address {
        return Err(invalid_alias("alias does not match the bound account"));
    }
    let descriptor = match protected_account_kind(view, runtime, bound).await? {
        None => return Err(invalid_alias("alias has no registered account")),
        Some(ProtectedKind::Derived) => {
            return Err(invalid_alias("derived alias registration is not finalized"));
        }
        Some(kind) => {
            let record = if kind == ProtectedKind::MlDsa {
                view.storage(&st::ML_DSA_ACCOUNTS, (bound,)).await?
            } else {
                view.storage(&st::ACCOUNTS, (bound,)).await?
            }
            .ok_or_else(|| invalid_alias("alias has no registered account"))?
            .to_json();
            let stored = record
                .get("descriptor")
                .ok_or_else(|| invalid_alias("'descriptor'"))?;
            descriptor_bytes(stored).map_err(|error| invalid_alias(&error.to_string()))?
        }
    };
    if descriptor_account(&descriptor).map_err(|error| invalid_alias(&error.to_string()))? != bound
    {
        return Err(invalid_alias("descriptor does not match the bound account"));
    }
    Ok(Recipient {
        address: rtao_core::keys::encode_hashed_receiving_address(&descriptor)
            .map_err(rtao_core::CoreError::from)?,
        account: bound,
        descriptor: Some(descriptor),
    })
}

/// Python `resolve_evm_deposit`: the truncated EVM alias of a native
/// account and the recipient its deposits reach.
#[doc(alias = "bittensor.evm.addresses.resolve_evm_deposit")]
pub async fn resolve_evm_deposit<V: View + ?Sized>(
    view: &V,
    runtime: &RuntimeHandle,
    native: AccountId,
    native_descriptor: Option<&[u8]>,
) -> Result<(String, Recipient), IntentError> {
    let alias = format!("0x{}", hex::encode(&native.0[..20]));
    let recipient = resolve_evm_recipient(view, runtime, &alias).await?;
    if recipient.descriptor.is_some() || recipient.account != mirror_account(&alias)? {
        if recipient.account != native {
            return Err(IntentError::validation(
                "this EVM alias belongs to another registered native account",
            ));
        }
    } else if native_descriptor.is_some() {
        return Err(IntentError::validation(
            "this hashed EVM deposit address is not active; first register and fund the wallet using `wallet transfer` with its complete receiving address on a supported chain",
        ));
    }
    Ok((alias, recipient))
}

/// Python `resolve_evm_funding_recipient` on runtimes without native EVM
/// deposits: native funding only where the destination is safe.
///
/// While hashed aliases are enabled, an unprotected address's mapping cannot
/// be guarded against a concurrent registration, so funding it is refused,
/// as Python. A registered hashed alias needs its receiving-address guard in
/// the same batch; rtao refuses that case here (D-154) and points to
/// `wallet transfer` with the complete receiving address, which carries it.
#[doc(alias = "bittensor.evm.addresses.resolve_evm_funding_recipient")]
pub async fn resolve_evm_funding_recipient<V: View + ?Sized>(
    view: &V,
    runtime: &RuntimeHandle,
    evm_address: &str,
) -> Result<Recipient, IntentError> {
    let recipient = resolve_evm_recipient(view, runtime, evm_address).await?;
    if recipient.descriptor.is_some() {
        return Err(IntentError::validation(format!(
            "this EVM alias credits a registered hashed account; fund it with `wallet transfer` to its complete receiving address {}",
            recipient.address
        )));
    }
    if hashed_accounts_enabled(view, runtime).await? == Some(true) {
        return Err(IntentError::validation(
            "native mirror funding is unavailable for an unprotected EVM address while hashed aliases are enabled; use `btcli evm fund` on a runtime supporting native EVM deposits, or send from an EVM wallet to the H160 address",
        ));
    }
    Ok(recipient)
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
    /// The legacy native mirror of the normalized EVM address; funding
    /// resolves the chain's active mapping ([`resolve_evm_recipient`]).
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
        format!(
            "fund EVM address {address} with {}",
            amount_summary(&self.amount_tao)
        )
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        let resolved = self.amount_tao.resolve(NetUid::ROOT, true)?;
        if native_deposit_supported(cx.view, cx.runtime).await? {
            let (address, _) = normalize_h160(&self.evm_address)?;
            let value = match resolved {
                ResolvedAmount::All => None,
                ResolvedAmount::Exact(amount) => Some(amount.tao()?.to_wire()),
            };
            let call = evm::deposit(DynValue::new(Value::Str(address)), value);
            return Ok(Built::new(call.compose(cx.runtime)?));
        }
        let recipient =
            resolve_evm_funding_recipient(cx.view, cx.runtime, &self.evm_address).await?;
        let destination = DynValue::new(Value::Bytes(recipient.account.0.to_vec()));
        let call = match resolved {
            ResolvedAmount::All => balances::transfer_all(destination, true),
            ResolvedAmount::Exact(amount) => {
                balances::transfer_keep_alive(destination, amount.tao()?.to_wire())
            }
        };
        Ok(Built::new(call.compose(cx.runtime)?))
    }

    async fn effects<V: View + ?Sized>(
        &self,
        cx: &PreflightCx<'_, V>,
    ) -> Result<Vec<String>, IntentError> {
        if native_deposit_supported(cx.view, cx.runtime).await? {
            return Ok(vec![self.summary()]);
        }
        let recipient =
            resolve_evm_funding_recipient(cx.view, cx.runtime, &self.evm_address).await?;
        Ok(vec![format!(
            "{} (native receiving address {})",
            self.summary(),
            recipient.address
        )])
    }

    async fn warnings<V: View + ?Sized>(
        &self,
        cx: &PreflightCx<'_, V>,
    ) -> Result<Vec<String>, IntentError> {
        let recipient = resolve_evm_recipient(cx.view, cx.runtime, &self.evm_address).await?;
        if recipient.account != mirror_account(&self.evm_address)? {
            return Ok(vec![
                "this EVM alias credits its registered hashed native account directly".into(),
            ]);
        }
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
        // Runtime withdrawal always debits the legacy mirror, including when
        // new EVM deposits already credit a registered native alias directly.
        let (address, _) = resolve_evm_deposit(cx.view, cx.runtime, coldkey, None).await?;
        let call = rtao_tx::pinned::compose_pinned("EVM", "withdraw", |_| async move {
            let amount = match resolved {
                ResolvedAmount::All => {
                    // A later multisig round rebuilding this call keeps the
                    // approved amount rather than re-reading the mirror.
                    let amount = match rtao_tx::pinned::approved_amount("EVM", "withdraw", "value")?
                    {
                        Some(rao) => {
                            rtao_primitives::Balance::<rtao_primitives::Tao>::from_rao(rao)
                        }
                        None => {
                            let mirror = mirror_account(&address)?;
                            rtao_tx::multisig::free_balance(cx.view, mirror).await?
                        }
                    };
                    if amount.is_zero() {
                        return Err(IntentError::validation(
                            "nothing to claim: the legacy EVM mirror is empty",
                        ));
                    }
                    amount
                }
                ResolvedAmount::Exact(amount) => amount.tao()?,
            };
            Ok(
                evm::withdraw(DynValue::new(Value::Str(address)), amount.to_wire())
                    .compose(cx.runtime)?,
            )
        })
        .await?;
        Ok(Built::new(call))
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
            "fund EVM address 0xabababababababababababababababababababab with τ1.250000000"
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
            // v488 runtimes credit EVM addresses natively (`EVM.deposit`),
            // resolving the active alias mapping during execution.
            let _ = compact;
            let mut expected = vec![22, 6];
            expected.extend_from_slice(&[0x11; 20]);
            expected.push(1);
            expected.extend_from_slice(&rao.to_le_bytes());
            assert_eq!(built.call.as_bytes(), expected);
            assert_eq!(CallBytes::decode(&expected, runtime)?, built.call);
            assert!(built.extras.is_empty());
            let decoded = decode(&built.call, runtime)?;
            assert_eq!(decoded["call_module"], "EVM");
            assert_eq!(decoded["call_function"], "deposit");
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
        let mut expected = vec![22, 6];
        expected.extend_from_slice(&[0x11; 20]);
        expected.push(0);
        assert_eq!(built.call.as_bytes(), expected);
        assert_eq!(CallBytes::decode(&expected, runtime)?, built.call);
        assert_eq!(
            decode(&built.call, runtime)?["call_args"][1]["value"],
            json!(null)
        );
        assert_eq!(intent.spend(), Spend::Unbounded);
        assert_eq!(
            intent.summary(),
            format!("fund EVM address {ADDRESS} with ALL TAO")
        );
        // Only the NativeDepositSupported constant is read, from metadata.
        assert!(
            recorded
                .touches()
                .iter()
                .all(|touch| touch.item == "NativeDepositSupported")
        );
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
        // Only the alias binding is read for an exact amount.
        assert!(
            recorded
                .touches()
                .iter()
                .all(|touch| touch.item == "EvmAliases")
        );
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
            assert_eq!(touches.len(), 2);
            assert_eq!(touches[0].item, "EvmAliases");
            assert_eq!(touches[1].pallet_or_api, "System");
            assert_eq!(touches[1].item, "Account");
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
                "nothing to claim: the legacy EVM mirror is empty"
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
        // The funding warning and the withdrawal each read the alias binding.
        assert_eq!(chain.requests().len(), requests + 2);
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn registered_hashed_aliases_resolve_to_their_native_account_and_fail_closed()
    -> anyhow::Result<()> {
        let key = rtao_core::keys::Keypair::from_seed(&[7; 32], rtao_core::keys::CRYPTO_HASHED)?;
        let descriptor = key.hashed_descriptor()?;
        let account = descriptor_account(&descriptor)?;
        let alias = format!("0x{}", hex::encode(&account.0[..20]));
        let seed = |bound: AccountId, record: bool| -> anyhow::Result<FakeState> {
            let mut state = FakeState::try_default()?;
            state.seed_python(
                "HashedAccounts",
                "EvmAliases",
                &[json!(alias)],
                json!(bound.to_string()),
            )?;
            if record {
                state.seed_python(
                    "HashedAccounts",
                    "Accounts",
                    &[json!(account.to_string())],
                    json!({
                        "descriptor": rtao_primitives::receiving::descriptor_value(&descriptor)?,
                        "generation": 1,
                        "commitment": format!("0x{}", "00".repeat(32)),
                    }),
                )?;
            }
            Ok(state)
        };
        let open = |state: FakeState| async move {
            let chain = FakeChain::from_state(state);
            let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
            config.track_heads = false;
            config.runtime_cache_dir = None;
            config.config_store = None;
            let client = Client::from_rpc(RpcClient::new(chain), config).await?;
            let snapshot = client.at(BlockRef::Finalized).await?;
            anyhow::Ok((client, snapshot))
        };
        let (client, snapshot) = open(seed(account, true)?).await?;
        let runtime = &snapshot.block().runtime;
        let recipient = resolve_evm_recipient(&snapshot, runtime, &alias).await?;
        assert_eq!(recipient.account, account);
        assert_eq!(recipient.descriptor.as_deref(), Some(&descriptor[..]));
        assert!(rtao_primitives::receiving::is_receiving_address(
            &recipient.address
        ));
        // The owner withdraws from its legacy mirror; another account cannot
        // claim the bound alias.
        assert_eq!(
            resolve_evm_deposit(&snapshot, runtime, account, None)
                .await?
                .0,
            alias
        );
        let funding = resolve_evm_funding_recipient(&snapshot, runtime, &alias)
            .await
            .expect_err("a hashed alias is funded through its receiving address");
        assert!(funding.to_string().contains("wallet transfer"), "{funding}");
        client.close().await;
        // An alias whose account is not registered fails closed.
        let (client, snapshot) = open(seed(account, false)?).await?;
        let error = resolve_evm_recipient(&snapshot, &snapshot.block().runtime, &alias)
            .await
            .expect_err("unregistered binding");
        assert_eq!(
            error.to_string(),
            "invalid protected EVM alias binding: alias has no registered account"
        );
        client.close().await;
        // A binding to an account with another prefix fails closed.
        let (client, snapshot) = open(seed(OTHER, true)?).await?;
        let error = resolve_evm_recipient(&snapshot, &snapshot.block().runtime, &alias)
            .await
            .expect_err("mismatched binding");
        assert_eq!(
            error.to_string(),
            "invalid protected EVM alias binding: alias does not match the bound account"
        );
        client.close().await;
        // An unbound address keeps the legacy mirror.
        let (client, snapshot) = open(FakeState::try_default()?).await?;
        let recipient =
            resolve_evm_recipient(&snapshot, &snapshot.block().runtime, ADDRESS).await?;
        assert_eq!(recipient.account.to_string(), MIRROR);
        assert!(recipient.descriptor.is_none());
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
