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
