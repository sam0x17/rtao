// Derived from RaoFoundation/subtensor sdk/python/bittensor/executor.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Executor policy, call identity and failure handling across external signing.

use rtao_client::{Client, ClientConfig, Inclusion, Network, SubmitOptions};
use rtao_primitives::{AccountId, CryptoType};
use rtao_signer::{KeypairSigner, MultiSignature, SignPurpose, Signer};
use rtao_testkit::{FakeChain, FakeState, Reply, rpc::RpcClient};
use rtao_tx::{
    ExecuteOptions, Executor, Policy, PrepareOptions, ProxyType, TxError, UnsignedExtrinsic,
};
use serde_json::json;
use std::time::Duration;

type TestResult = Result<(), Box<dyn std::error::Error>>;

async fn fixture() -> Result<(Executor, FakeChain, KeypairSigner), Box<dyn std::error::Error>> {
    let fake = FakeChain::from_state(FakeState::try_default()?);
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    let client = Client::from_rpc(RpcClient::new(fake.clone()), config).await?;
    Ok((
        Executor::new(client),
        fake,
        KeypairSigner::from_seed(&[7; 32], CryptoType::Ed25519)?,
    ))
}

fn prepare_options() -> PrepareOptions {
    PrepareOptions {
        crypto_type: CryptoType::Ed25519,
        period: None,
        ..Default::default()
    }
}

fn pool_options() -> ExecuteOptions {
    ExecuteOptions {
        wait_for_inclusion: false,
        wait_for_finalization: false,
        ..Default::default()
    }
}

async fn prepared(
    executor: &Executor,
    signer: &KeypairSigner,
) -> Result<UnsignedExtrinsic, Box<dyn std::error::Error>> {
    let runtime = executor.client().refresh_runtime().await?;
    let call = rtao_runtime::calls::system::remark(vec![1, 2, 3]).compose(&runtime)?;
    Ok(executor
        .prepare_call(call, signer.account(), &prepare_options())
        .await?)
}

fn no_author(fake: &FakeChain) {
    assert!(
        !fake
            .requests()
            .iter()
            .any(|request| request.method.starts_with("author_"))
    );
}

#[tokio::test]
async fn imported_payload_submission_uses_current_policy_and_per_call_replacement() -> TestResult {
    let (executor, fake, signer) = fixture().await?;
    let executor = executor.with_policy(Policy::default());
    // Public preparation is permitted even when submission requires an opt-in.
    let unsigned = prepared(&executor, &signer).await?;
    let imported = executor
        .client()
        .import_unsigned_json(
            &serde_json::to_vec(&unsigned.to_json()?)?,
            &SubmitOptions::default(),
        )
        .await?;
    let signature = signer
        .sign(&imported.signing_request(SignPurpose::Extrinsic))
        .await?;
    executor.client().nonces().pin(&signer.account(), 80);
    assert!(matches!(
        executor
            .submit_signature(&imported, signature, &pool_options())
            .await,
        Err(TxError::Policy(_))
    ));
    no_author(&fake);
    assert_eq!(
        executor
            .client()
            .nonces()
            .cached(&signer.account().to_string())
            .await,
        Some(80)
    );
    let allow = ExecuteOptions {
        policy: Some(Policy {
            allow_raw_calls: true,
            ..Default::default()
        }),
        ..pool_options()
    };
    let receipt = executor
        .submit_signature(&imported, signature, &allow)
        .await?;
    assert!(receipt.success);
    assert_eq!(receipt.inclusion, Inclusion::PoolAccepted);
    assert_eq!(
        fake.state().lock().map_err(|_| "state")?.submissions.len(),
        1
    );
    let executor = executor.with_policy(Policy {
        allow_raw_calls: true,
        ..Default::default()
    });
    assert!(matches!(
        executor
            .submit_signature(
                &imported,
                signature,
                &ExecuteOptions {
                    policy: Some(Policy::default()),
                    ..pool_options()
                }
            )
            .await,
        Err(TxError::Policy(_))
    ));
    assert_eq!(
        fake.state().lock().map_err(|_| "state")?.submissions.len(),
        1
    );
    executor.client().close().await;
    Ok(())
}

#[tokio::test]
async fn proxy_sudo_and_multisig_calls_keep_exact_bytes_account_and_resolved_receipt() -> TestResult
{
    let (executor, fake, signer) = fixture().await?;
    let runtime = executor.client().refresh_runtime().await?;
    let inner = rtao_runtime::calls::system::remark(vec![1, 2, 3]).compose(&runtime)?;
    let proxy = rtao_client::compose::proxy(&runtime, AccountId([9; 32]), None, inner.clone())?;
    let sudo = rtao_client::compose::sudo(&runtime, inner.clone())?;
    let multisig =
        rtao_client::compose::as_multi_threshold_1(&runtime, &[AccountId([8; 32])], inner)?;
    for call in [proxy, sudo, multisig] {
        let unsigned = executor
            .prepare_call(call.clone(), signer.account(), &prepare_options())
            .await?;
        let imported = executor
            .client()
            .import_unsigned_json(
                &serde_json::to_vec(&unsigned.to_json()?)?,
                &SubmitOptions::default(),
            )
            .await?;
        assert_eq!(imported.call(), &call);
        assert_eq!(imported.address(), signer.account());
        let signature = signer
            .sign(&imported.signing_request(SignPurpose::Extrinsic))
            .await?;
        let (signed, hash) = executor
            .client()
            .attach_signature(&imported, signature)
            .await?;
        assert!(signed.ends_with(call.as_bytes()));
        let receipt = executor
            .submit_signature(&imported, signature, &ExecuteOptions::default())
            .await?;
        assert!(receipt.success);
        assert_eq!(receipt.inclusion, Inclusion::Finalized);
        assert!(receipt.fate_unknown.is_none());
        assert_eq!(
            fake.state()
                .lock()
                .map_err(|_| "state")?
                .submissions
                .last()
                .ok_or("submitted bytes")?,
            signed.as_ref()
        );
        let resolved = executor
            .client()
            .resolve_extrinsic(hash, receipt.block_hash.ok_or("block hash")?, true)
            .await?;
        assert_eq!(receipt.extrinsic_id, resolved.extrinsic_id);
        assert_eq!(receipt.events, resolved.events);
        assert!(receipt.data.is_empty());
    }
    assert_eq!(
        fake.state().lock().map_err(|_| "state")?.submissions.len(),
        3
    );
    executor.client().close().await;
    Ok(())
}

#[tokio::test]
async fn external_submission_honors_the_reviewed_call_constraint() -> TestResult {
    let (executor, fake, signer) = fixture().await?;
    let unsigned = prepared(&executor, &signer).await?;
    let signature = signer
        .sign(&unsigned.signing_request(SignPurpose::Extrinsic))
        .await?;
    let runtime = executor.client().refresh_runtime().await?;
    let different = rtao_runtime::calls::system::remark(vec![9]).compose(&runtime)?;
    executor.client().nonces().pin(&signer.account(), 80);
    let refused = executor
        .submit_signature(
            &unsigned,
            signature,
            &ExecuteOptions {
                expected_call: Some(different),
                ..pool_options()
            },
        )
        .await;
    assert!(matches!(refused, Err(TxError::Intent(_))));
    no_author(&fake);
    assert_eq!(
        executor
            .client()
            .nonces()
            .cached(&signer.account().to_string())
            .await,
        Some(80)
    );
    let receipt = executor
        .submit_signature(
            &unsigned,
            signature,
            &ExecuteOptions {
                expected_call: Some(unsigned.call().clone()),
                ..pool_options()
            },
        )
        .await?;
    assert!(receipt.success);
    executor.client().close().await;
    Ok(())
}

#[tokio::test]
async fn post_signature_proxy_and_shield_options_refuse_without_author_or_nonce_mutation()
-> TestResult {
    let (executor, fake, signer) = fixture().await?;
    let unsigned = prepared(&executor, &signer).await?;
    let signature = signer
        .sign(&unsigned.signing_request(SignPurpose::Extrinsic))
        .await?;
    executor.client().nonces().pin(&signer.account(), 80);
    for opts in [
        ExecuteOptions {
            proxy_for: Some(AccountId([9; 32])),
            ..pool_options()
        },
        ExecuteOptions {
            force_proxy_type: Some(ProxyType::Any),
            ..pool_options()
        },
        ExecuteOptions {
            shielded: true,
            ..pool_options()
        },
    ] {
        let error = executor
            .submit_signature(&unsigned, signature, &opts)
            .await
            .err()
            .ok_or("must refuse")?;
        assert!(matches!(error, TxError::Intent(_)));
        assert!(
            error
                .to_string()
                .contains("wrappers to be composed before preparation")
        );
        no_author(&fake);
        assert_eq!(
            executor
                .client()
                .nonces()
                .cached(&signer.account().to_string())
                .await,
            Some(80)
        );
    }
    executor.client().close().await;
    Ok(())
}

#[tokio::test]
async fn wrong_signing_account_signature_and_algorithm_refuse_before_explicit_lease() -> TestResult
{
    let (executor, fake, signer) = fixture().await?;
    let unsigned = prepared(&executor, &signer).await?;
    let stranger = KeypairSigner::from_seed(&[8; 32], CryptoType::Ed25519)?;
    let valid = signer
        .sign(&unsigned.signing_request(SignPurpose::Extrinsic))
        .await?;
    let wrong_account = stranger
        .sign(&unsigned.signing_request(SignPurpose::Extrinsic))
        .await?;
    let wrong_algorithm = MultiSignature {
        crypto: CryptoType::Sr25519,
        bytes: valid.bytes,
    };
    let mut altered = valid;
    altered.bytes[0] ^= 1;
    executor.client().nonces().pin(&signer.account(), 80);
    for signature in [wrong_account, wrong_algorithm, altered] {
        assert!(
            executor
                .submit_signature(&unsigned, signature, &pool_options())
                .await
                .is_err()
        );
        no_author(&fake);
        assert_eq!(
            executor
                .client()
                .nonces()
                .cached(&signer.account().to_string())
                .await,
            Some(80)
        );
    }
    executor.client().close().await;
    Ok(())
}

#[tokio::test]
async fn each_live_runtime_stamp_change_refuses_prepared_signatures_without_replanning()
-> TestResult {
    for (spec, transaction) in [(479, 1), (478, 2)] {
        let (executor, fake, signer) = fixture().await?;
        let unsigned = prepared(&executor, &signer).await?;
        let signature = signer
            .sign(&unsigned.signing_request(SignPurpose::Extrinsic))
            .await?;
        fake.mutate_state(|state| state.set_result("state_getRuntimeVersion", json!({
            "specName":"node-subtensor", "specVersion":spec, "transactionVersion":transaction, "apis":[]
        })));
        assert!(
            executor
                .submit_signature(
                    &unsigned,
                    signature,
                    &ExecuteOptions {
                        retries: 3,
                        ..pool_options()
                    }
                )
                .await
                .is_err()
        );
        no_author(&fake);
        assert_eq!(
            executor
                .client()
                .nonces()
                .cached(&signer.account().to_string())
                .await,
            None
        );
        executor.client().close().await;
    }
    Ok(())
}

#[tokio::test]
async fn public_preparation_keeps_cache_and_signed_nonce_era_tip_ignore_later_period_options()
-> TestResult {
    let (executor, fake, signer) = fixture().await?;
    fake.mutate_state(|state| state.set_nonce(signer.account().to_string(), 7));
    executor.client().nonces().pin(&signer.account(), 80);
    let unsigned = prepared(&executor, &signer).await?;
    assert_eq!(unsigned.nonce(), 7);
    let runtime = executor.client().refresh_runtime().await?;
    let call = unsigned.call().clone();
    let explicit = executor
        .prepare_call(
            call,
            signer.account(),
            &PrepareOptions {
                nonce: Some(42),
                tip: 7,
                ..prepare_options()
            },
        )
        .await?;
    assert_eq!(explicit.nonce(), 42);
    assert_eq!(explicit.tip(), 7);
    assert_eq!(explicit.tip_asset_id(), None);
    assert_eq!(
        executor
            .client()
            .nonces()
            .cached(&signer.account().to_string())
            .await,
        Some(80)
    );
    let signature = signer
        .sign(&explicit.signing_request(SignPurpose::Extrinsic))
        .await?;
    let (signed, _) = executor
        .client()
        .attach_signature(&explicit, signature)
        .await?;
    let receipt = executor
        .submit_signature(
            &explicit,
            signature,
            &ExecuteOptions {
                period: Some(64),
                ..pool_options()
            },
        )
        .await?;
    assert!(receipt.success);
    let submitted = fake
        .state()
        .lock()
        .map_err(|_| "state")?
        .submissions
        .clone();
    assert_eq!(submitted.as_slice(), &[signed.to_vec()]);
    let decoded =
        rtao_core::DynValue::new(runtime.core().decode_extrinsic(&submitted[0], true)?).to_json();
    assert_eq!(decoded["nonce"], json!(42));
    assert_eq!(decoded["tip"], json!(7));
    assert_eq!(decoded["era"], json!("00"));
    assert_eq!(
        executor
            .client()
            .nonces()
            .cached(&signer.account().to_string())
            .await,
        Some(42)
    );
    executor.client().close().await;
    Ok(())
}

#[tokio::test]
async fn zero_preparation_or_submission_deadlines_refuse_before_author() -> TestResult {
    let (executor, fake, signer) = fixture().await?;
    let unsigned = prepared(&executor, &signer).await?;
    let signature = signer
        .sign(&unsigned.signing_request(SignPurpose::Extrinsic))
        .await?;
    executor.client().nonces().pin(&signer.account(), 80);
    assert!(
        executor
            .prepare_call(
                unsigned.call().clone(),
                signer.account(),
                &PrepareOptions {
                    deadline: Some(Duration::ZERO),
                    ..prepare_options()
                }
            )
            .await
            .is_err()
    );
    assert_eq!(
        executor
            .client()
            .nonces()
            .cached(&signer.account().to_string())
            .await,
        Some(80)
    );
    for opts in [
        ExecuteOptions {
            deadline: Some(Duration::ZERO),
            ..pool_options()
        },
        ExecuteOptions {
            submit_deadline: Some(Duration::ZERO),
            ..pool_options()
        },
    ] {
        assert!(
            executor
                .submit_signature(&unsigned, signature, &opts)
                .await
                .is_err()
        );
    }
    no_author(&fake);
    executor.client().close().await;
    Ok(())
}

#[tokio::test]
async fn missing_author_ack_preserves_exact_attempted_hash_and_clears_nonce_cache() -> TestResult {
    let (executor, fake, signer) = fixture().await?;
    let unsigned = prepared(&executor, &signer).await?;
    let signature = signer
        .sign(&unsigned.signing_request(SignPurpose::Extrinsic))
        .await?;
    let (signed, expected) = executor
        .client()
        .attach_signature(&unsigned, signature)
        .await?;
    fake.mutate_state(|state| {
        state.set_handler("author_submitExtrinsic", |state, request| {
            let _accepted = state.dispatch(request);
            Reply::silent()
        })
    });
    let receipt = executor
        .submit_signature(
            &unsigned,
            signature,
            &ExecuteOptions {
                retries: 3,
                submit_deadline: Some(Duration::from_millis(50)),
                deadline: Some(Duration::from_secs(1)),
                ..pool_options()
            },
        )
        .await?;
    assert!(!receipt.success);
    assert_eq!(receipt.inclusion, Inclusion::Unobserved);
    assert_eq!(receipt.fate_unknown, Some(expected));
    assert!(receipt.block_hash.is_none());
    assert!(receipt.extrinsic_id.is_none());
    assert!(receipt.events.is_empty());
    assert_eq!(
        fake.state()
            .lock()
            .map_err(|_| "state")?
            .submissions
            .as_slice(),
        &[signed.to_vec()]
    );
    assert_eq!(
        executor
            .client()
            .nonces()
            .cached(&signer.account().to_string())
            .await,
        None
    );
    executor.client().close().await;
    Ok(())
}
