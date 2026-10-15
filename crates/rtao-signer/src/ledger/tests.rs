// Derived from RaoFoundation/subtensor sdk/python/tests/unit/test_ledger_signer.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Offline ports of the pinned Python Ledger signer tests and safety regressions.
//!
//! Python's missing-context test does not apply: every SigningRequest carries
//! its borrowed context, so there is no previously cached context to forget.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::{
    sync::{Condvar, Mutex, OnceLock},
    thread::ThreadId,
    time::Duration,
};

use parity_scale_codec::Decode;
use rtao_core::{codec::Value, extrinsic::TxParams, runtime::Runtime};
use serde_json::Value as JsonValue;

use super::*;
use crate::{SignPurpose, SignerPayloadJson};

#[derive(Debug, Clone, PartialEq, Eq)]
enum DeviceCall {
    Version,
    Address(u32, u32, u16, bool),
    Sign {
        account: u32,
        index: u32,
        payload: Vec<u8>,
        proof: Vec<u8>,
    },
}

struct FakeLedger {
    calls: Mutex<Vec<(DeviceCall, ThreadId)>>,
    signature: Mutex<Result<Vec<u8>, CoreError>>,
    address_error: Mutex<Option<CoreError>>,
    version_error: Mutex<Option<CoreError>>,
    confirmed_key: Mutex<[u8; 32]>,
    sign_gate: Option<Arc<SignGate>>,
}

impl Default for FakeLedger {
    fn default() -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            signature: Mutex::new(Ok([vec![0], vec![0x11; 64]].concat())),
            address_error: Mutex::new(None),
            version_error: Mutex::new(None),
            confirmed_key: Mutex::new([1; 32]),
            sign_gate: None,
        }
    }
}

impl FakeLedger {
    fn record(&self, call: DeviceCall) {
        self.calls
            .lock()
            .unwrap()
            .push((call, std::thread::current().id()));
    }

    fn calls(&self) -> Vec<DeviceCall> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .map(|(call, _)| call.clone())
            .collect()
    }
}

impl LedgerBackend for FakeLedger {
    fn app_version(&self) -> Result<(u16, u16, u16), CoreError> {
        self.record(DeviceCall::Version);
        if let Some(error) = self.version_error.lock().unwrap().clone() {
            return Err(error);
        }
        Ok((100, 0, 5))
    }

    fn address(
        &self,
        account: u32,
        index: u32,
        ss58_prefix: u16,
        confirm: bool,
    ) -> Result<LedgerAddress, CoreError> {
        self.record(DeviceCall::Address(account, index, ss58_prefix, confirm));
        if let Some(error) = self.address_error.lock().unwrap().clone() {
            return Err(error);
        }
        Ok(LedgerAddress {
            public_key: if confirm {
                *self.confirmed_key.lock().unwrap()
            } else {
                [1; 32]
            },
            ss58_address: "5FakeLedgerAddress".into(),
        })
    }

    fn sign(
        &self,
        account: u32,
        index: u32,
        payload: &[u8],
        proof: &[u8],
    ) -> Result<Vec<u8>, CoreError> {
        self.record(DeviceCall::Sign {
            account,
            index,
            payload: payload.to_vec(),
            proof: proof.to_vec(),
        });
        if let Some(gate) = &self.sign_gate {
            if let Some(started) = gate.started.lock().unwrap().take() {
                let _ = started.send(());
            }
            let released = gate.released.lock().unwrap();
            let (released, result) = gate
                .changed
                .wait_timeout_while(released, Duration::from_secs(5), |released| !*released)
                .unwrap();
            if result.timed_out() && !*released {
                return Err(CoreError::Device("fake approval timed out".into()));
            }
        }
        self.signature.lock().unwrap().clone()
    }
}

struct SignGate {
    started: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    released: Mutex<bool>,
    changed: Condvar,
}

struct ReleaseSign(Arc<SignGate>);

impl Drop for ReleaseSign {
    fn drop(&mut self) {
        *self.0.released.lock().unwrap() = true;
        self.0.changed.notify_all();
    }
}

fn context() -> SigningContext {
    static CONTEXT: OnceLock<SigningContext> = OnceLock::new();
    CONTEXT
        .get_or_init(|| {
            let golden: JsonValue = serde_json::from_str(include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../vendor/upstream/golden.json"
            )))
            .unwrap();
            let encoded = hex::decode(
                golden["metadata"]["v15_hex"]
                    .as_str()
                    .unwrap()
                    .trim_start_matches("0x"),
            )
            .unwrap();
            let metadata_bytes = Option::<Vec<u8>>::decode(&mut encoded.as_slice())
                .unwrap()
                .unwrap();
            let genesis_hash = hex::decode(
                golden["network"]["genesis_hash"]
                    .as_str()
                    .unwrap()
                    .trim_start_matches("0x"),
            )
            .unwrap()
            .try_into()
            .unwrap();
            SigningContext {
                metadata_bytes,
                chain_info: ChainInfo {
                    spec_version: 461,
                    spec_name: "node-subtensor".into(),
                    base58_prefix: 42,
                    decimals: 9,
                    token_symbol: "TAO".into(),
                },
                transaction_version: 1,
                genesis_hash,
            }
        })
        .clone()
}

fn proof_vector() -> JsonValue {
    serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../vendor/upstream/ledger_proof_vector.json"
    )))
    .unwrap()
}

struct Parts {
    call: Vec<u8>,
    extra: Vec<u8>,
    additional: Vec<u8>,
    payload: Vec<u8>,
}

impl Parts {
    fn vector() -> Self {
        let vector = proof_vector();
        let decode = |name: &str| hex::decode(vector[name].as_str().unwrap()).unwrap();
        let call = decode("call_data_hex");
        let extra = decode("included_in_extrinsic_hex");
        let additional = decode("included_in_signed_data_hex");
        let payload = [&*call, &*extra, &*additional].concat();
        Self {
            call,
            extra,
            additional,
            payload,
        }
    }

    async fn sign(
        &self,
        signer: &LedgerSigner,
        context: &SigningContext,
    ) -> Result<MultiSignature, SignerError> {
        let payload_json = SignerPayloadJson::new();
        signer
            .sign(&SigningRequest {
                call: &self.call,
                included_in_extrinsic: &self.extra,
                included_in_signed_data: &self.additional,
                payload: &self.payload,
                payload_json: &payload_json,
                context,
                purpose: SignPurpose::Extrinsic,
            })
            .await
    }
}

async fn signer(fake: Arc<FakeLedger>, options: LedgerOptions) -> LedgerSigner {
    LedgerSigner::from_backend(fake, options).await.unwrap()
}

// pyt: sdk/python/tests/unit/test_ledger_signer.py::test_satisfies_the_signing_protocols
#[tokio::test]
async fn satisfies_signing_protocols_and_offloads_each_device_operation() {
    let worker_thread = std::thread::current().id();
    let fake = Arc::new(FakeLedger::default());
    let signer = signer(fake.clone(), LedgerOptions::default()).await;
    let contract: &dyn Signer = &signer;
    assert_eq!(contract.account(), AccountId([1; 32]));
    assert_eq!(contract.crypto_type(), CryptoType::Ed25519);
    assert_eq!(
        contract.caps(),
        SignerCaps::METADATA_HASH | SignerCaps::UNSIGNED_TX
    );
    assert_eq!(contract.metadata_constants(), Some((9, "TAO".into())));
    assert_eq!(signer.public_key(), &[1; 32]);
    assert_eq!(signer.ss58_address(), "5FakeLedgerAddress");
    assert_eq!(signer.ss58_format(), 42);
    assert_eq!(signer.app_version().await.unwrap(), (100, 0, 5));
    assert_eq!(
        signer.confirm_address().await.unwrap(),
        "5FakeLedgerAddress"
    );
    assert_eq!(
        fake.calls(),
        [
            DeviceCall::Address(0, 0, 42, false),
            DeviceCall::Version,
            DeviceCall::Address(0, 0, 42, true)
        ]
    );
    assert!(
        fake.calls
            .lock()
            .unwrap()
            .iter()
            .all(|(_, thread)| *thread != worker_thread)
    );
    assert!(format!("{signer:?}").contains("m/44'/354'/0'/0'/0'"));
}

// pyt: sdk/python/tests/unit/test_ledger_signer.py::test_metadata_digest_matches_pinned_vector
#[tokio::test]
async fn metadata_digest_matches_pinned_python_and_core_vector() {
    let signer = signer(Arc::new(FakeLedger::default()), LedgerOptions::default()).await;
    assert_eq!(
        hex::encode(signer.metadata_digest(&context()).await.unwrap()),
        "affbcc25b1cb6847bc94dcf3a7f35fb8d6a5c59d9d47da92063a9399a5fc11c0"
    );
}

// pyt: sdk/python/tests/unit/test_ledger_signer.py::test_raw_sign_is_refused
#[tokio::test]
async fn raw_sign_is_refused_without_device_io() {
    let fake = Arc::new(FakeLedger::default());
    let signer = signer(fake.clone(), LedgerOptions::default()).await;
    assert_eq!(
        signer.sign_raw(b"arbitrary message").await,
        Err(SignerError::RawSigningUnsupported)
    );
    assert_eq!(fake.calls(), [DeviceCall::Address(0, 0, 42, false)]);
}

// pyt: sdk/python/tests/unit/test_ledger_signer.py::test_sign_unsigned_extrinsic_ships_payload_and_proof
#[tokio::test]
async fn ledger_request_parity_matches_committed_core_request_vector() {
    // This vector is committed core evidence. A fresh Python fake-device dump
    // and device/emulator verification are separate, still absent evidence.
    let fake = Arc::new(FakeLedger::default());
    let signer = signer(fake.clone(), LedgerOptions::default()).await;
    let parts = Parts::vector();
    let signature = parts.sign(&signer, &context()).await.unwrap();
    assert_eq!(signature.crypto, CryptoType::Ed25519);
    assert_eq!(signature.bytes, [0x11; 64]);
    assert_eq!(
        fake.calls(),
        [
            DeviceCall::Address(0, 0, 42, false),
            DeviceCall::Sign {
                account: 0,
                index: 0,
                payload: [&*parts.call, &*parts.extra, &*parts.additional].concat(),
                proof: hex::decode(proof_vector()["rust_proof_hex"].as_str().unwrap()).unwrap(),
            }
        ]
    );
}

#[tokio::test]
async fn token_options_preserve_runtime_spec_name_and_context_ss58_prefix() {
    let fake = Arc::new(FakeLedger::default());
    let options = LedgerOptions {
        account: 2,
        index: 7,
        ss58_format: 2,
        decimals: 12,
        token_symbol: "UNIT".into(),
    };
    let signer = signer(fake.clone(), options).await;
    let mut context = context();
    context.chain_info.spec_name = "runtime-renamed".into();
    context.chain_info.base58_prefix = 99;
    let mut expected = context.chain_info.clone();
    expected.decimals = 12;
    expected.token_symbol = "UNIT".into();
    assert_eq!(
        signer.metadata_digest(&context).await.unwrap(),
        rtao_core::digest::metadata_digest(&context.metadata_bytes, &expected).unwrap()
    );
    Parts::vector().sign(&signer, &context).await.unwrap();
    let calls = fake.calls();
    assert_eq!(calls[0], DeviceCall::Address(2, 7, 2, false));
    let DeviceCall::Sign {
        account,
        index,
        proof,
        ..
    } = &calls[1]
    else {
        panic!("expected signature request");
    };
    assert_eq!((*account, *index), (2, 7));
    // SCALE ExtraInfo tail: LE version, compact strings, LE prefix, decimals.
    let mut tail = 461_u32.to_le_bytes().to_vec();
    tail.extend_from_slice(b"\x3cruntime-renamed\x63\x00\x0c\x10UNIT");
    assert!(proof.ends_with(&tail));
}

#[tokio::test]
async fn long_mortal_batch_sends_unhashed_parts_instead_of_the_prepared_hash() {
    let context = context();
    let runtime = Runtime::parse(&context.metadata_bytes, 461, 1, 42).unwrap();
    let transfer = Parts::vector().call;
    let call = runtime
        .compose_call(
            "Utility",
            "batch_all",
            &Value::record(vec![(
                "calls".into(),
                Value::List(vec![Value::Bytes(transfer); 10]),
            )]),
        )
        .unwrap();
    let params = TxParams {
        era: Value::record(vec![
            ("period".into(), Value::Uint(64)),
            ("current".into(), Value::Uint(50)),
        ]),
        nonce: 7,
        tip: 0,
        tip_asset_id: None,
        genesis_hash: context.genesis_hash,
        era_block_hash: [0x42; 32],
        metadata_hash: Some(
            rtao_core::digest::metadata_digest(&context.metadata_bytes, &context.chain_info)
                .unwrap(),
        ),
    };
    let (extra, additional) = runtime.signature_payload_parts(&params).unwrap();
    let payload = runtime.signature_payload(&call, &params).unwrap();
    assert_eq!(payload.len(), 32);
    let parts = Parts {
        call,
        extra,
        additional,
        payload,
    };
    let fake = Arc::new(FakeLedger::default());
    let signer = signer(fake.clone(), LedgerOptions::default()).await;
    parts.sign(&signer, &context).await.unwrap();
    let calls = fake.calls();
    let DeviceCall::Sign { payload, proof, .. } = &calls[1] else {
        panic!("expected signature request");
    };
    assert!(payload.len() > 256);
    assert_eq!(
        payload,
        &[&*parts.call, &*parts.extra, &*parts.additional].concat()
    );
    assert_ne!(payload, &parts.payload);
    assert_eq!(
        proof,
        &rtao_core::digest::generate_extrinsic_proof(
            &parts.call,
            &parts.extra,
            &parts.additional,
            &context.metadata_bytes,
            &context.chain_info
        )
        .unwrap()
    );
    assert_ne!(&parts.extra[..2], &[0, 0]);
}

#[tokio::test]
async fn malformed_or_wrong_scheme_device_replies_are_refused() {
    let fake = Arc::new(FakeLedger::default());
    let signer = signer(fake.clone(), LedgerOptions::default()).await;
    let parts = Parts::vector();
    let context = context();
    for reply in [vec![], vec![0; 64], vec![0; 66], vec![1; 65], vec![2; 65]] {
        *fake.signature.lock().unwrap() = Ok(reply);
        assert!(matches!(
            parts.sign(&signer, &context).await,
            Err(SignerError::InvalidSignature(_))
        ));
    }
}

#[tokio::test]
async fn device_rejection_keeps_the_core_error_message() {
    let fake = Arc::new(FakeLedger::default());
    let signer = signer(fake.clone(), LedgerOptions::default()).await;
    let rejection = CoreError::Device("the request was rejected on the device".into());
    *fake.signature.lock().unwrap() = Err(rejection.clone());
    assert_eq!(
        Parts::vector().sign(&signer, &context()).await,
        Err(SignerError::Core(rejection.clone()))
    );
    *fake.version_error.lock().unwrap() = Some(rejection.clone());
    assert_eq!(
        signer.app_version().await,
        Err(SignerError::Core(rejection.clone()))
    );
    *fake.address_error.lock().unwrap() = Some(rejection.clone());
    assert_eq!(
        signer.confirm_address().await,
        Err(SignerError::Core(rejection))
    );
}

#[tokio::test]
async fn failed_initial_address_derivation_does_not_construct_a_signer() {
    let fake = Arc::new(FakeLedger::default());
    let failure = CoreError::Device("app not recognized".into());
    *fake.address_error.lock().unwrap() = Some(failure.clone());
    assert_eq!(
        LedgerSigner::from_backend(fake, LedgerOptions::default())
            .await
            .unwrap_err(),
        SignerError::Core(failure)
    );
}

#[tokio::test]
async fn changed_confirmed_device_account_is_refused() {
    let fake = Arc::new(FakeLedger::default());
    let signer = signer(fake.clone(), LedgerOptions::default()).await;
    *fake.confirmed_key.lock().unwrap() = [2; 32];
    assert!(matches!(
        signer.confirm_address().await,
        Err(SignerError::Core(CoreError::Device(message)))
        if message.contains("differs from the signer's account")
    ));
}

#[tokio::test]
async fn invalid_metadata_fails_before_contacting_the_signing_device() {
    let fake = Arc::new(FakeLedger::default());
    let signer = signer(fake.clone(), LedgerOptions::default()).await;
    let mut context = context();
    context.metadata_bytes = vec![0; 16];
    assert!(matches!(
        Parts::vector().sign(&signer, &context).await,
        Err(SignerError::Core(CoreError::Codec(_)))
    ));
    assert_eq!(fake.calls(), [DeviceCall::Address(0, 0, 42, false)]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_signing_keeps_the_operation_lock_until_the_device_returns() {
    let (started, entered) = tokio::sync::oneshot::channel();
    let gate = Arc::new(SignGate {
        started: Mutex::new(Some(started)),
        released: Mutex::new(false),
        changed: Condvar::new(),
    });
    let release = ReleaseSign(gate.clone());
    let fake = Arc::new(FakeLedger {
        sign_gate: Some(gate),
        ..FakeLedger::default()
    });
    let first = signer(fake.clone(), LedgerOptions::default()).await;
    let second = signer(
        fake.clone(),
        LedgerOptions {
            account: 1,
            ..LedgerOptions::default()
        },
    )
    .await;
    let signing = tokio::spawn(async move { Parts::vector().sign(&first, &context()).await });
    tokio::time::timeout(Duration::from_secs(5), entered)
        .await
        .unwrap()
        .unwrap();
    signing.abort();
    assert!(signing.await.unwrap_err().is_cancelled());
    // Check the guard directly as well as observing a second operation. This
    // would fail immediately if cancelling the async caller released its lock;
    // a delayed CI task cannot make the exclusion assertion pass by itself.
    let lock_retained = matches!(
        DEVICE_OPERATION.try_lock(),
        Err(std::sync::TryLockError::WouldBlock)
    );
    let mut version = tokio::spawn(async move { second.app_version().await });
    let excluded = tokio::time::timeout(Duration::from_millis(30), &mut version)
        .await
        .is_err();
    let calls_while_pending = fake.calls();
    // Release before assertions so a regression cannot strand the blocking
    // worker while the Tokio runtime is being torn down by a panic.
    drop(release);
    let reply = tokio::time::timeout(Duration::from_secs(5), version)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(lock_retained);
    assert!(excluded);
    assert!(matches!(
        calls_while_pending.last(),
        Some(DeviceCall::Sign { .. })
    ));
    assert_eq!(reply, (100, 0, 5));
    assert_eq!(fake.calls().last(), Some(&DeviceCall::Version));
}
