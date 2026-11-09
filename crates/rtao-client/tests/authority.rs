//! Hashed and post-quantum signing from chain-read authority (D-147).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use rtao_client::{
    BlockRef, Client, ClientConfig, ClientError, Inclusion, Network, NoncePolicy, SubmitFailure,
    SubmitOptions,
};
use rtao_core::keys::{CRYPTO_HASHED, CRYPTO_HASHED_ED25519, CRYPTO_MLDSA65_FIXED, Keypair};
use rtao_primitives::{AuthorityType, MLDSA_PROOF_LENGTH, receiving::descriptor_value};
use rtao_signer::{
    AuthoritySigner, AuthorizationProof, AuthorizationRequest, BoxFuture, KeypairAuthoritySigner,
};
use rtao_testkit::{FakeChain, FakeState, SubmitScript};
use serde_json::json;
use subxt_rpcs::RpcClient;

fn config() -> ClientConfig {
    let mut cfg = ClientConfig::new(Network::resolve_with("local", |_| None).unwrap()).unwrap();
    cfg.track_heads = false;
    cfg.runtime_cache_dir = None;
    cfg.config_store = None;
    cfg
}

fn signer(crypto: u8) -> KeypairAuthoritySigner {
    KeypairAuthoritySigner::new(Keypair::from_seed(&[7; 32], crypto).unwrap()).unwrap()
}

/// Enabled hashed accounts, the signer registered at `generation` and its
/// confirmed nonce `nonce` (also the pool's next index).
fn state(signer: &dyn AuthoritySigner, generation: u64, nonce: u64) -> FakeState {
    let mut state = FakeState::default();
    let account = signer.account();
    state
        .seed_python("AdminUtils", "HashedAccountsEnabled", &[], json!(true))
        .unwrap();
    state
        .seed_python(
            "System",
            "Account",
            &[json!(account)],
            json!({"nonce": nonce}),
        )
        .unwrap();
    state.set_nonce(account.to_string(), nonce);
    if !signer.authority_type().is_stateless() {
        let storage = signer.authority_type().records_storage();
        let commitment = signer.commitment(generation).unwrap();
        state
            .seed_python(
                "HashedAccounts",
                storage,
                &[json!(account)],
                json!({
                    "descriptor": descriptor_value(signer.descriptor()).unwrap(),
                    "generation": generation,
                    "commitment": format!("0x{}", hex::encode(commitment)),
                }),
            )
            .unwrap();
    }
    state
}

async fn connect(state: FakeState) -> (Client, FakeChain) {
    let fake = FakeChain::from_state(state);
    let client = Client::from_rpc(RpcClient::new(fake.clone()), config())
        .await
        .unwrap();
    (client, fake)
}

async fn remark(client: &Client) -> rtao_runtime::CallBytes {
    rtao_runtime::calls::system::remark(vec![1, 2, 3])
        .compose(&client.refresh_runtime().await.unwrap())
        .unwrap()
}

fn refusal(error: ClientError) -> String {
    match error {
        ClientError::SigningDisabled { reason } => reason,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[tokio::test]
async fn rotating_accounts_sign_at_the_chain_generation_and_submit() {
    for crypto in [CRYPTO_HASHED, CRYPTO_HASHED_ED25519] {
        let signer = signer(crypto);
        let (client, fake) = connect(state(&signer, 3, 5)).await;
        let snapshot = client
            .hashed_snapshot(&signer, BlockRef::Best)
            .await
            .unwrap();
        assert_eq!((snapshot.generation, snapshot.nonce), (3, 5));
        assert_eq!(snapshot.commitment, signer.commitment(3).unwrap());
        let signed = client
            .sign_authorized(
                remark(&client).await,
                &signer,
                &SubmitOptions::default(),
                &snapshot,
                0,
            )
            .await
            .unwrap();
        assert_eq!(signed.nonce, 5);
        // The core decodes the extrinsic with its rotating signer address.
        let runtime = client.refresh_runtime().await.unwrap();
        let decoded = runtime
            .core()
            .decode_extrinsic(&signed.bytes, true)
            .unwrap();
        assert!(format!("{decoded:?}").contains("System"));
        // A Shield inner proof is the next generation at the next nonce.
        let inner = client
            .sign_authorized(
                remark(&client).await,
                &signer,
                &SubmitOptions::default(),
                &snapshot,
                1,
            )
            .await
            .unwrap();
        assert_eq!(inner.nonce, 6);
        assert!(
            client
                .sign_authorized(
                    remark(&client).await,
                    &signer,
                    &SubmitOptions::default(),
                    &snapshot,
                    2,
                )
                .await
                .is_err()
        );
        fake.mutate_state(|state| {
            state.queue_submit(SubmitScript::InBlockThenFinalized { events: json!([]) })
        });
        let report = client
            .submit_authorized(remark(&client).await, &signer, &SubmitOptions::default())
            .await
            .unwrap();
        assert_eq!(report.inclusion, Inclusion::Finalized);
    }
}

#[tokio::test]
async fn rotating_accounts_refuse_unverifiable_or_pending_authority() {
    let signer = signer(CRYPTO_HASHED);
    // Disabled on chain.
    let mut disabled = state(&signer, 0, 0);
    disabled
        .seed_python("AdminUtils", "HashedAccountsEnabled", &[], json!(false))
        .unwrap();
    let (client, _) = connect(disabled).await;
    let error = client
        .hashed_snapshot(&signer, BlockRef::Finalized)
        .await
        .unwrap_err();
    assert_eq!(
        refusal(error),
        "hashed accounts are not enabled on this chain"
    );
    // Not registered.
    let other =
        KeypairAuthoritySigner::new(Keypair::from_seed(&[8; 32], CRYPTO_HASHED).unwrap()).unwrap();
    let (client, fake) = connect(state(&signer, 0, 0)).await;
    let error = client
        .hashed_snapshot(&other, BlockRef::Finalized)
        .await
        .unwrap_err();
    assert!(refusal(error).starts_with("hashed account is not registered"));
    // The chain's commitment is not this key's at the recorded generation.
    let account = signer.account();
    fake.mutate_state(|state| {
        state
            .seed_python(
                "HashedAccounts",
                "Accounts",
                &[json!(account)],
                json!({
                    "descriptor": descriptor_value(signer.descriptor()).unwrap(),
                    "generation": 2,
                    "commitment": format!("0x{}", hex::encode(signer.commitment(1).unwrap())),
                }),
            )
            .unwrap()
    });
    let snapshot = client
        .hashed_snapshot(&signer, BlockRef::Finalized)
        .await
        .unwrap();
    let error = client
        .sign_authorized(
            remark(&client).await,
            &signer,
            &SubmitOptions::default(),
            &snapshot,
            0,
        )
        .await
        .unwrap_err();
    assert_eq!(
        refusal(error),
        "derived hashed signing key does not match the current on-chain commitment"
    );
    // A pending transaction (pool ahead of the confirmed nonce) and an
    // explicit nonce other than the confirmed one are refused before sending.
    let (client, fake) = connect(state(&signer, 0, 4)).await;
    fake.mutate_state(|state| state.set_nonce(account.to_string(), 5));
    let error = client
        .submit_authorized(remark(&client).await, &signer, &SubmitOptions::default())
        .await
        .unwrap_err();
    let SubmitFailure::Client(error) = error else {
        panic!("expected a client refusal");
    };
    assert!(refusal(error).starts_with("hashed account has a pending transaction"));
    fake.mutate_state(|state| state.set_nonce(account.to_string(), 4));
    let opts = SubmitOptions {
        nonce: NoncePolicy::Explicit(9),
        ..SubmitOptions::default()
    };
    let error = client
        .submit_authorized(remark(&client).await, &signer, &opts)
        .await
        .unwrap_err();
    let SubmitFailure::Client(error) = error else {
        panic!("expected a client refusal");
    };
    assert!(refusal(error).starts_with("hashed accounts must use the current on-chain nonce"));
    assert!(
        !fake
            .requests()
            .iter()
            .any(|request| request.method.starts_with("author_submit"))
    );
    // A snapshot of another account is refused.
    let error = client
        .sign_authorized(
            remark(&client).await,
            &other,
            &SubmitOptions::default(),
            &client
                .hashed_snapshot(&signer, BlockRef::Best)
                .await
                .unwrap(),
            0,
        )
        .await
        .unwrap_err();
    assert_eq!(
        refusal(error),
        "hashed signing snapshot does not match the pinned account authority"
    );
}

#[tokio::test]
async fn standard_post_quantum_accounts_need_no_registration_and_pipeline() {
    let signer = signer(CRYPTO_MLDSA65_FIXED);
    assert_eq!(signer.authority_type(), AuthorityType::MlDsa65Fixed);
    let (client, fake) = connect(state(&signer, 0, 2)).await;
    let snapshot = client.prepare_hashed_snapshot(&signer).await.unwrap();
    assert_eq!((snapshot.generation, snapshot.nonce), (0, 2));
    assert_eq!(snapshot.commitment, signer.descriptor()[2..]);
    let signed = client
        .sign_authorized(
            remark(&client).await,
            &signer,
            &SubmitOptions::default(),
            &snapshot,
            0,
        )
        .await
        .unwrap();
    assert!(signed.bytes.len() > MLDSA_PROOF_LENGTH);
    for _ in 0..2 {
        fake.mutate_state(|state| {
            state.queue_submit(SubmitScript::InBlockThenFinalized { events: json!([]) })
        });
    }
    client
        .submit_authorized(remark(&client).await, &signer, &SubmitOptions::default())
        .await
        .unwrap();
    // The lease advanced locally: the next transaction takes the next nonce.
    assert_eq!(
        client.nonces().cached(&signer.account().to_string()).await,
        Some(2)
    );
}

/// Signs a generation other than the requested one: its proof is well formed
/// but must not verify against the chain-read authority.
struct WrongGeneration(KeypairAuthoritySigner);

impl AuthoritySigner for WrongGeneration {
    fn account(&self) -> rtao_primitives::AccountId {
        self.0.account()
    }
    fn authority_type(&self) -> AuthorityType {
        self.0.authority_type()
    }
    fn descriptor(&self) -> &[u8] {
        self.0.descriptor()
    }
    fn commitment(&self, generation: u64) -> Result<Vec<u8>, rtao_signer::SignerError> {
        self.0.commitment(generation)
    }
    fn sign_authorization<'a>(
        &'a self,
        request: &'a AuthorizationRequest<'a>,
    ) -> BoxFuture<'a, Result<AuthorizationProof, rtao_signer::SignerError>> {
        Box::pin(async move {
            let shifted = AuthorizationRequest {
                generation: request.generation + 1,
                ..*request
            };
            self.0.sign_authorization(&shifted).await
        })
    }
}

#[tokio::test]
async fn proofs_that_do_not_verify_against_chain_authority_are_never_sent() {
    let signer = WrongGeneration(signer(CRYPTO_HASHED));
    let (client, fake) = connect(state(&signer, 1, 0)).await;
    let error = client
        .submit_authorized(remark(&client).await, &signer, &SubmitOptions::default())
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains(
            "authorization proof does not verify against the account's on-chain authority"
        ),
        "{error}"
    );
    assert!(
        !fake
            .requests()
            .iter()
            .any(|request| request.method.starts_with("author_submit"))
    );
}

#[test]
fn keypair_authority_signers_refuse_classical_keys() {
    let classical = Keypair::from_seed(&[7; 32], rtao_core::keys::CRYPTO_SR25519).unwrap();
    assert!(KeypairAuthoritySigner::new(classical).is_err());
    let signer = signer(CRYPTO_HASHED);
    assert!(
        signer
            .expect_identity(signer.account(), &[1, 1, 0])
            .is_err()
    );
    signer
        .expect_identity(signer.account(), signer.descriptor())
        .unwrap();
    assert!(!format!("{signer:?}").contains("seed"));
}

fn decoded_nonce(client_runtime: &rtao_runtime::RuntimeHandle, bytes: &[u8]) -> u128 {
    use rtao_core::value_ext::ValueExt;
    client_runtime
        .core()
        .decode_extrinsic(bytes, true)
        .unwrap()
        .field("nonce")
        .and_then(ValueExt::as_u128)
        .unwrap()
}

#[tokio::test]
async fn authority_signers_submit_through_the_ordinary_signer_path() {
    // D-149: a KeypairAuthoritySigner is a Signer whose authority() routes
    // Client::submit to the authorization path.
    for crypto in [CRYPTO_HASHED, CRYPTO_MLDSA65_FIXED] {
        let signer = signer(crypto);
        let (client, fake) = connect(state(&signer, 3, 5)).await;
        fake.mutate_state(|state| {
            state.queue_submit(SubmitScript::InBlockThenFinalized { events: json!([]) })
        });
        let as_signer: &dyn rtao_signer::Signer = &signer;
        assert!(as_signer.authority().is_some());
        let report = client
            .submit(remark(&client).await, as_signer, &SubmitOptions::default())
            .await
            .unwrap();
        assert_eq!(report.inclusion, Inclusion::Finalized);
        let submitted = fake.mutate_state(|state| state.submissions.clone());
        assert_eq!(submitted.len(), 1);
        let runtime = client.refresh_runtime().await.unwrap();
        assert_eq!(decoded_nonce(&runtime, &submitted[0]), 5);
        assert!(submitted[0].len() > signer.authority_type().proof_length());
    }
}

#[tokio::test]
async fn rotating_accounts_refuse_caller_owned_leases() {
    let signer = signer(CRYPTO_HASHED);
    let (client, _fake) = connect(state(&signer, 0, 1)).await;
    let lease = client.nonces().lease(&signer.account()).await.unwrap();
    let error = client
        .submit_leased(
            remark(&client).await,
            &signer,
            lease,
            &SubmitOptions::default(),
        )
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("sign at their confirmed nonce"),
        "{error}"
    );
}

#[tokio::test]
async fn shield_pairs_sign_both_proofs_from_one_snapshot() {
    for crypto in [CRYPTO_HASHED, CRYPTO_MLDSA65_FIXED] {
        let signer = signer(crypto);
        let mut state = state(&signer, 3, 5);
        state
            .seed_constant("Aura", "SlotDuration", json!(1))
            .unwrap();
        state
            .seed_typed("MevShield", "NextKey", (), vec![0u8; 1184])
            .unwrap();
        let (client, fake) = connect(state).await;
        client
            .submit_shielded(
                remark(&client).await,
                &signer,
                &rtao_client::ShieldOptions {
                    wait: rtao_client::Wait::Pool,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let submitted = fake.mutate_state(|state| state.submissions.clone());
        assert_eq!(submitted.len(), 1, "only the carrier is broadcast");
        let runtime = client.refresh_runtime().await.unwrap();
        // The carrier takes nonce n; its sealed inner, n + 1.
        assert_eq!(decoded_nonce(&runtime, &submitted[0]), 5);
        assert_eq!(
            client.nonces().cached(&signer.account().to_string()).await,
            Some(6)
        );
    }
}

#[tokio::test]
async fn fee_probes_price_the_signing_scheme_proof_length() {
    let signer = signer(CRYPTO_MLDSA65_FIXED);
    let (client, fake) = connect(state(&signer, 0, 0)).await;
    let call = remark(&client).await;
    let probe_length = |fake: &FakeChain, since: usize| {
        let requests = fake.requests();
        let request = requests[since..]
            .iter()
            .rev()
            .find(|request| {
                request.method == "state_call"
                    && request.params[0] == "TransactionPaymentApi_query_info"
            })
            .expect("fee probe")
            .clone();
        request.params[1].as_str().unwrap().len()
    };
    let start = fake.requests().len();
    client
        .estimate_weight(
            call.clone(),
            signer.account(),
            rtao_primitives::CryptoType::Sr25519,
        )
        .await
        .unwrap();
    let classical = probe_length(&fake, start);
    let start = fake.requests().len();
    let as_signer: &dyn rtao_signer::Signer = &signer;
    client
        .estimate_weight_as(
            call,
            signer.account(),
            rtao_client::ProbeSigner::of(as_signer),
        )
        .await
        .unwrap();
    let authority = probe_length(&fake, start);
    // Hex characters: the proof replaces the 64-byte signature.
    assert!(
        authority - classical >= 2 * (MLDSA_PROOF_LENGTH - 64),
        "{classical} {authority}"
    );
}
