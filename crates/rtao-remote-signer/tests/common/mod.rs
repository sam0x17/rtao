//! Golden-vector fixtures shared by the offline test targets.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicU64, AtomicUsize, Ordering},
};

use rtao_btauth::{Clock, ClockError};
use rtao_core::digest::ChainInfo;
use rtao_primitives::{AccountId, CryptoType};
use rtao_remote_signer::wire::SignRequest;
use rtao_runtime::RuntimeHandle;
use rtao_signer::{
    BoxFuture, KeypairSigner, MultiSignature, SignPurpose, Signer, SignerError, SignerPayloadJson,
    SigningContext, SigningRequest,
};
use rtao_testkit::fixtures::{Golden, decode_hex};
use serde_json::{Value as Json, json};

/// 2025-07-09T16:00:00Z in Unix nanoseconds (MDX vector timestamp).
pub const NOW: u64 = 1_752_076_800_000_000_000;
pub const SPEC_NAME: &str = "node-subtensor";
pub const GENESIS_HEX: &str = "0x1dc00e76a13fc48b9c932e076622c5920295afaad4fd805c284a5cf3222ce452";
pub const ERA_BLOCK_HEX: &str =
    "0xd9f2d5c7e6b387e32b3f41d7808094cc966fbbbbd4709d96471c52d9b1a2b2aa";
pub const CALL_HEX: &str =
    "0x0503008eaf04151687736326c9fea17e25fc5287613693c912909cb226aa4794f26a48e5c0";
pub const SIGNED_EXTENSIONS: [&str; 13] = [
    "CheckNonZeroSender",
    "CheckSpecVersion",
    "CheckTxVersion",
    "CheckGenesis",
    "CheckMortality",
    "CheckNonce",
    "CheckWeight",
    "ChargeTransactionPayment",
    "SudoTransactionExtension",
    "CheckShieldedTxValidity",
    "SubtensorTransactionExtension",
    "DrandPriority",
    "CheckMetadataHash",
];

pub fn golden() -> &'static Golden {
    static GOLDEN: OnceLock<Golden> = OnceLock::new();
    GOLDEN.get_or_init(|| {
        Golden::from_slice(include_bytes!("../../../../vendor/upstream/golden.json")).unwrap()
    })
}

pub fn runtime() -> RuntimeHandle {
    static RUNTIME: OnceLock<RuntimeHandle> = OnceLock::new();
    RUNTIME
        .get_or_init(|| RuntimeHandle::new(golden().runtime().unwrap()).unwrap())
        .clone()
}

pub fn hex32(text: &str) -> [u8; 32] {
    decode_hex(text).unwrap().try_into().unwrap()
}

pub fn genesis() -> [u8; 32] {
    hex32(GENESIS_HEX)
}

pub fn key(uri: &str) -> KeypairSigner {
    KeypairSigner::from_uri(uri, CryptoType::Sr25519).unwrap()
}
pub fn alice() -> KeypairSigner {
    key("//Alice")
}
pub fn bob() -> KeypairSigner {
    key("//Bob")
}
pub fn charlie() -> KeypairSigner {
    key("//Charlie")
}
pub fn dave() -> KeypairSigner {
    key("//Dave")
}

pub fn context() -> SigningContext {
    SigningContext {
        metadata_bytes: runtime().core().metadata_bytes.clone(),
        chain_info: ChainInfo {
            spec_version: 461,
            spec_name: SPEC_NAME.into(),
            base58_prefix: 42,
            decimals: 9,
            token_symbol: "TAO".into(),
        },
        transaction_version: 1,
        genesis_hash: genesis(),
    }
}

/// Owned views of one prepared transaction.
#[derive(Clone, Debug)]
pub struct Vector {
    pub call: Vec<u8>,
    pub extra: Vec<u8>,
    pub additional: Vec<u8>,
    pub payload: Vec<u8>,
    pub payload_json: SignerPayloadJson,
    pub context: SigningContext,
    pub purpose: SignPurpose,
}

impl Vector {
    pub fn signing_request(&self) -> SigningRequest<'_> {
        SigningRequest {
            call: &self.call,
            included_in_extrinsic: &self.extra,
            included_in_signed_data: &self.additional,
            payload: &self.payload,
            payload_json: &self.payload_json,
            context: &self.context,
            purpose: self.purpose,
        }
    }
    pub fn body(&self) -> SignRequest {
        SignRequest::from_signing_request(
            &self.signing_request(),
            alice().account(),
            CryptoType::Sr25519,
        )
    }
    pub fn body_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(&self.body()).unwrap()
    }
}

fn payload_json(era: &str, nonce: &str, block_hash: &str, call_hex: &str) -> SignerPayloadJson {
    let value = json!({
        "address": "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY",
        "blockHash": block_hash,
        "genesisHash": GENESIS_HEX,
        "method": call_hex,
        "nonce": nonce,
        "specVersion": "0x000001cd",
        "tip": "0x00000000000000000000000000000000",
        "transactionVersion": "0x00000001",
        "era": era,
        "version": 4,
        "signedExtensions": SIGNED_EXTENSIONS,
        "mode": 0,
        "metadataHash": null,
    });
    match value {
        Json::Object(map) => map,
        _ => unreachable!(),
    }
}

/// V1: mortal era {period 64, current 66}, nonce 3, tip 0, no metadata hash.
pub fn v1() -> Vector {
    let call = decode_hex(CALL_HEX).unwrap();
    let extra = decode_hex("0x25000c0000").unwrap();
    let additional = [
        decode_hex("0xcd01000001000000").unwrap(),
        genesis().to_vec(),
        hex32(ERA_BLOCK_HEX).to_vec(),
        vec![0],
    ]
    .concat();
    let payload = decode_hex(&golden().signature_payloads[1].payload_hex).unwrap();
    Vector {
        call,
        extra,
        additional,
        payload,
        payload_json: payload_json("0x2500", "0x00000003", ERA_BLOCK_HEX, CALL_HEX),
        context: context(),
        purpose: SignPurpose::Extrinsic,
    }
}

/// V2: immortal, nonce 7.
pub fn v2() -> Vector {
    let call = decode_hex(CALL_HEX).unwrap();
    let extra = decode_hex("0x001c0000").unwrap();
    let additional = [
        decode_hex("0xcd01000001000000").unwrap(),
        genesis().to_vec(),
        genesis().to_vec(),
        vec![0],
    ]
    .concat();
    let payload = decode_hex(&golden().signature_payloads[0].payload_hex).unwrap();
    Vector {
        call,
        extra,
        additional,
        payload,
        payload_json: payload_json("0x00", "0x00000007", GENESIS_HEX, CALL_HEX),
        context: context(),
        purpose: SignPurpose::Extrinsic,
    }
}

/// A consistent vector for an arbitrary call under V1's era and nonce.
pub fn with_call(call: Vec<u8>) -> Vector {
    let mut vector = v1();
    let call_hex = format!("0x{}", hex::encode(&call));
    vector.payload_json.insert("method".into(), json!(call_hex));
    let mut data = [call.as_slice(), &vector.extra, &vector.additional].concat();
    if data.len() > 256 {
        data = rtao_remote_signer::wire::blake2_256(&data).to_vec();
    }
    vector.payload = data;
    vector.call = call;
    vector
}

/// `System.remark(remark)` encoded with the golden runtime's indices.
pub fn remark(remark: &[u8]) -> Vec<u8> {
    let rt = runtime();
    let mut call = vec![
        rt.pallet_index("System").unwrap(),
        rt.call_index("System", "remark").unwrap(),
    ];
    rtao_core::codec::encode::compact(remark.len() as u128, &mut call).unwrap();
    call.extend_from_slice(remark);
    call
}

/// Deterministic, strictly increasing signing clock.
#[derive(Debug)]
pub struct StepClock(pub AtomicU64);
impl StepClock {
    pub fn at(ns: u64) -> Arc<Self> {
        Arc::new(Self(AtomicU64::new(ns)))
    }
}
impl Clock for StepClock {
    fn now_ns(&self) -> Result<u64, ClockError> {
        Ok(self.0.fetch_add(1, Ordering::Relaxed))
    }
}

/// Fixed verification clock.
#[derive(Debug)]
pub struct FixedClock(pub u64);
impl Clock for FixedClock {
    fn now_ns(&self) -> Result<u64, ClockError> {
        Ok(self.0)
    }
}

/// A backend signer that counts signing calls.
#[derive(Clone)]
pub struct CountingSigner {
    pub inner: KeypairSigner,
    pub calls: Arc<AtomicUsize>,
}
impl CountingSigner {
    pub fn new(inner: KeypairSigner) -> Self {
        Self {
            inner,
            calls: Arc::new(AtomicUsize::new(0)),
        }
    }
    pub fn count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}
impl Signer for CountingSigner {
    fn account(&self) -> AccountId {
        self.inner.account()
    }
    fn crypto_type(&self) -> CryptoType {
        self.inner.crypto_type()
    }
    fn sign<'a>(
        &'a self,
        request: &'a SigningRequest<'a>,
    ) -> BoxFuture<'a, Result<MultiSignature, SignerError>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inner.sign(request)
    }
}

pub fn verify(payload: &[u8], signature: &[u8], account: AccountId) -> bool {
    rtao_core::keys::verify_strict(payload, signature, &account.to_ss58(42), 1).unwrap()
}
