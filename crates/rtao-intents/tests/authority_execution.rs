// Derived from RaoFoundation/subtensor sdk/python/bittensor/executor.py @ b6a81d144078eac18bd25c3af7cb31579bb670f3 (Apache-2.0); modified by rtao.
//! The executor plans, quotes and submits for hashed and post-quantum
//! signers through the ordinary key-source seam (D-149).
#![cfg(feature = "transfer")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use anyhow::Result;
use rtao_client::{Client, ClientConfig, Network};
use rtao_core::{
    keys::{CRYPTO_HASHED, CRYPTO_MLDSA65_FIXED, Keypair},
    value_ext::ValueExt,
};
use rtao_intents::AnyIntent;
use rtao_primitives::{AccountId, receiving::descriptor_value};
use rtao_signer::{
    AuthoritySigner, BoxFuture, DynKeySource, KeyView, KeypairAuthoritySigner, Signer, SignerError,
    SignerRole,
};
use rtao_testkit::{FakeChain, FakeState, SubmitScript, rpc::RpcClient};
use rtao_tx::{ExecuteOptions, PlanOptions};
use serde_json::json;

const TAO: u64 = 1_000_000_000;
const DEST: AccountId = AccountId([3; 32]);

struct AuthorityKeys(KeypairAuthoritySigner);
impl KeyView for AuthorityKeys {
    fn hotkey_address(&self) -> Result<AccountId, SignerError> {
        Err(SignerError::KeyUnavailable(SignerRole::Hotkey))
    }
    fn hotkey_public_key(&self) -> Result<[u8; 32], SignerError> {
        Err(SignerError::KeyUnavailable(SignerRole::Hotkey))
    }
    fn coldkey_address(&self) -> Result<AccountId, SignerError> {
        Ok(AuthoritySigner::account(&self.0))
    }
    fn authority_descriptor(&self, role: SignerRole) -> Option<Vec<u8>> {
        (role == SignerRole::Coldkey).then(|| self.0.descriptor().to_vec())
    }
}
impl DynKeySource for AuthorityKeys {
    fn signer_for<'a>(
        &'a self,
        role: SignerRole,
    ) -> BoxFuture<'a, Result<Arc<dyn Signer>, SignerError>> {
        Box::pin(async move {
            if role != SignerRole::Coldkey {
                return Err(SignerError::KeyUnavailable(role));
            }
            Ok(Arc::new(self.0.clone()) as Arc<dyn Signer>)
        })
    }
}

async fn fixture(crypto: u8) -> Result<(rtao_tx::Executor, FakeChain, AuthorityKeys)> {
    let signer = KeypairAuthoritySigner::new(Keypair::from_seed(&[7; 32], crypto)?)?;
    let account = AuthoritySigner::account(&signer);
    let mut state = FakeState::try_default()?;
    state.fee = 73;
    state.seed_python("AdminUtils", "HashedAccountsEnabled", &[], json!(true))?;
    state.seed_python(
        "System",
        "Account",
        &[json!(account.to_string())],
        json!({"nonce": 4, "data": {"free": 100 * TAO}}),
    )?;
    state.set_nonce(account.to_string(), 4);
    if !signer.authority_type().is_stateless() {
        state.seed_python(
            "HashedAccounts",
            signer.authority_type().records_storage(),
            &[json!(account.to_string())],
            json!({
                "descriptor": descriptor_value(signer.descriptor())?,
                "generation": 2,
                "commitment": format!("0x{}", hex::encode(signer.commitment(2)?)),
            }),
        )?;
    }
    let chain = FakeChain::from_state(state);
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    let client = Client::from_rpc(RpcClient::new(chain.clone()), config).await?;
    Ok((rtao_intents::executor(client), chain, AuthorityKeys(signer)))
}

fn transfer() -> AnyIntent {
    AnyIntent::from_tool(
        "transfer",
        json!({"dest_ss58": DEST.to_string(), "amount_tao": "1"}),
    )
    .unwrap()
}

#[tokio::test]
async fn plans_quote_the_proof_and_execution_submits_an_authorized_extrinsic() -> Result<()> {
    for crypto in [CRYPTO_HASHED, CRYPTO_MLDSA65_FIXED] {
        let (executor, chain, keys) = fixture(crypto).await?;
        let intent = transfer();
        let start = chain.requests().len();
        let plan = executor
            .plan_dyn(intent.as_dyn(), &keys, &PlanOptions::default())
            .await?;
        assert!(plan.ok(), "{:?}", plan.violations);
        let requests = chain.requests();
        let probe = requests[start..]
            .iter()
            .find(|request| {
                request.method == "state_call"
                    && request.params[0] == "TransactionPaymentApi_query_info"
            })
            .expect("fee probe");
        let probe = hex::decode(probe.params[1].as_str().unwrap().trim_start_matches("0x"))?;
        // The probe carries a placeholder proof of the scheme's length, not a
        // 64-byte signature (the trailing four bytes are its length).
        assert!(probe.len() > keys.0.authority_type().proof_length() + 4);
        chain.mutate_state(|state| {
            state.queue_submit(SubmitScript::InBlockThenFinalized {
                events: json!([{"phase":"ApplyExtrinsic","extrinsic_idx":0,"module_id":"System","event_id":"ExtrinsicSuccess","attributes":{"dispatch_info":{"weight":{"ref_time":1,"proof_size":0},"class":"Normal","pays_fee":"Yes"}},"topics":[]}]),
            })
        });
        let result = executor
            .execute_dyn(intent.as_dyn(), &keys, &ExecuteOptions::default())
            .await?;
        assert!(result.success, "{result:?}");
        let submitted = chain.mutate_state(|state| state.submissions.clone());
        assert_eq!(submitted.len(), 1);
        let runtime = executor.client().refresh_runtime().await?;
        let decoded = runtime.core().decode_extrinsic(&submitted[0], true)?;
        assert_eq!(decoded.field("nonce").and_then(ValueExt::as_u128), Some(4));
        assert!(submitted[0].len() > keys.0.authority_type().proof_length());
        executor.client().close().await;
    }
    Ok(())
}

// Python v488 `test_mldsa_orders.py` (D-160): a hashed, ML-DSA or Falcon owner
// approves, cancels and relays its own account-approved orders through the
// ordinary authority signing path; classical signatures and account approvals
// mix in one relay; another origin cannot approve the owner's terms.
#[cfg(feature = "limit_orders")]
#[tokio::test]
async fn authority_owners_approve_cancel_and_relay_account_approved_orders() -> Result<()> {
    use rtao_intents::limit_orders::PreparedOrder;
    let mut cryptos = vec![rtao_core::keys::CRYPTO_HASHED_ED25519];
    cryptos.extend(7..=12);
    for crypto in cryptos {
        let (executor, chain, keys) = fixture(crypto).await?;
        let owner = AuthoritySigner::account(&keys.0);
        let runtime = executor.client().refresh_runtime().await?;
        let genesis = executor.client().genesis().0;
        let order = json!({"V1": {
            "signer": owner.to_string(), "hotkey": DEST.to_string(), "netuid": 1,
            "order_type": "LimitBuy", "amount": 100, "limit_price": 1_000_000_000_u64,
            "expiry": 2_000_000_000_000_u64, "fee_rate": 0, "fee_recipient": DEST.to_string(),
            "relayer": [DEST.to_string()], "max_slippage": 1000, "chain_id": 42,
            "partial_fills_enabled": true,
        }});
        let prepared = PreparedOrder::prepare(&runtime, genesis, 9, &order)?;
        assert_eq!(prepared.owner()?, owner);
        let approve = AnyIntent::from_tool(
            "authorize_order",
            serde_json::to_value(prepared.authorize())?,
        )?;
        let cancel =
            AnyIntent::from_tool("cancel_order", serde_json::to_value(prepared.cancel())?)?;
        let classical = json!({
            "order": {"V1": {"signer": DEST.to_string(), "hotkey": DEST.to_string(), "netuid": 1,
                "order_type": "LimitBuy", "amount": 1, "limit_price": 1, "expiry": 1, "fee_rate": 0,
                "fee_recipient": DEST.to_string(), "relayer": null, "max_slippage": null,
                "chain_id": 42, "partial_fills_enabled": false}},
            "signature": {"Sr25519": format!("0x{}", "11".repeat(64))},
            "partial_fill": null,
        });
        let relay = AnyIntent::from_tool(
            "execute_orders",
            json!({"orders": [classical, prepared.account_approved(Some(10))]}),
        )?;
        let netted = AnyIntent::from_tool(
            "execute_batched_orders",
            json!({"orders": [prepared.account_approved(None)], "netuid": 1}),
        )?;
        for intent in [&approve, &cancel, &relay, &netted] {
            let plan = executor
                .plan_dyn(intent.as_dyn(), &keys, &PlanOptions::default())
                .await?;
            assert!(plan.ok(), "{crypto}: {:?}", plan.violations);
            assert_eq!(plan.signer_address, owner);
        }
        // The owner's approval is an authorized extrinsic carrying the pinned
        // terms (prepared bytes) and the owner's account.
        chain.mutate_state(|state| {
            state.queue_submit(SubmitScript::InBlockThenFinalized {
                events: json!([{"phase":"ApplyExtrinsic","extrinsic_idx":0,"module_id":"System","event_id":"ExtrinsicSuccess","attributes":{"dispatch_info":{"weight":{"ref_time":1,"proof_size":0},"class":"Normal","pays_fee":"Yes"}},"topics":[]}]),
            })
        });
        let result = executor
            .execute_dyn(approve.as_dyn(), &keys, &ExecuteOptions::default())
            .await?;
        assert!(result.success, "{crypto}: {result:?}");
        let submitted = chain.mutate_state(|state| state.submissions.clone());
        assert_eq!(submitted.len(), 1);
        let decoded =
            rtao_core::DynValue::new(runtime.core().decode_extrinsic(&submitted[0], true)?)
                .to_json();
        assert_eq!(
            decoded["call"]["call_function"], "authorize_order",
            "{crypto}"
        );
        assert!(submitted[0].len() > keys.0.authority_type().proof_length());
        let encoded = hex::encode(&prepared.encoded);
        assert!(hex::encode(&submitted[0]).contains(&encoded), "{crypto}");
        // Another account cannot approve the owner's terms.
        let (other, _, other_keys) = fixture(if crypto == 7 { 8 } else { 7 }).await?;
        let error = other
            .plan_dyn(approve.as_dyn(), &other_keys, &PlanOptions::default())
            .await
            .expect_err("another origin must refuse");
        assert!(error.to_string().contains("order owner"), "{error}");
        other.client().close().await;
        executor.client().close().await;
    }
    Ok(())
}
