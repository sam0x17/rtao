//! Golden vectors (spec-epistula.md section 11) and Python reference parity.
//!
//! `tests/vectors/python.json` was produced by running the BitMind and Apex
//! verifiers locally (`ci/interop/epistula/run.sh`). Set
//! `EPISTULA_PY_VECTORS` to check a freshly generated file instead.
//!
//! Oracle coverage differs by preset. `bitmind` and `apex` are compared with
//! verdicts of the pinned upstream Python code. `spec` is compared with a
//! transcription of the epistula.sybil.com reference verifier inside the
//! harness, not with code published as a package. `rtao` and `gobt` have no
//! oracle of their own: they reuse BitMind's verdicts plus the hand-written
//! differences in `deviation`. No Go verifier has produced a gobt verdict;
//! treat the gobt verify side as unverified until the CI gobt job runs.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::{
    error::Error,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use http::{HeaderMap, HeaderName, HeaderValue};
use rtao_btauth::{BoxFuture, InMemoryNonceStore, NonceStoreConfig, WrappedPolicy};
use rtao_compat_epistula::{
    BodyDigest, Dialect, EpistulaError, IntervalFormat, MessageForm, ReplayPolicy, SignOptions,
    Verifier, apex::canonical_json, header, interval, request_message, secret_message,
    sign_headers,
};
use rtao_primitives::{AccountId, CryptoType};
use rtao_signer::{KeypairSigner, MultiSignature, Signer, SignerError, SigningRequest};
use serde_json::Value;
use sha2::{Digest, Sha256};

type TestResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;

const TS: u64 = 1_700_000_000_123;
const UUID: &str = "8b0e5f5e-3f6c-4a7e-9c1d-2b9a4f6d7e10";
const ALICE: &str = "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY";
const BOB: &str = "5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty";
const BOB_PREFIX0: &str = "14E5nqKAp3oAJcmzgZhUD2RcptBeUBScxKHgJKU4HPNcKVf3";
const JSON_BODY: &[u8] = br#"{"key": "value"}"#;

fn sha_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn alice() -> KeypairSigner {
    KeypairSigner::from_uri("//Alice", CryptoType::Sr25519).unwrap()
}

fn bob() -> AccountId {
    BOB.parse().unwrap()
}

/// Records every raw message, then signs it with a real key.
struct RecordingSigner {
    inner: KeypairSigner,
    messages: Mutex<Vec<String>>,
}

impl RecordingSigner {
    fn new() -> Self {
        Self {
            inner: alice(),
            messages: Mutex::new(Vec::new()),
        }
    }
    fn take(&self) -> Vec<String> {
        std::mem::take(&mut self.messages.lock().unwrap())
    }
}

impl Signer for RecordingSigner {
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
        self.inner.sign(request)
    }
    fn sign_raw<'a>(&'a self, message: &'a [u8]) -> BoxFuture<'a, Result<[u8; 64], SignerError>> {
        self.messages
            .lock()
            .unwrap()
            .push(String::from_utf8(message.to_vec()).unwrap());
        self.inner.sign_raw(message)
    }
}

fn opts<'a>(body: &'a [u8], signed_for: Option<&'a str>) -> SignOptions<'a> {
    SignOptions {
        digest: BodyDigest::Raw(body),
        signed_for,
        now_ms: TS,
        uuid: Some(UUID),
    }
}

#[test]
fn body_digests_match_spec_table() {
    for (body, digest) in [
        (
            &b""[..],
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        ),
        (
            JSON_BODY,
            "9724c1e20e6e3e4d7f57ed25f9d4efb006e508590d528c90da597f6a775c13e5",
        ),
        (
            b"Hello World",
            "a591a6d40bf420404a011733cfb7b190d62c65bf0bcda32b57b277d9ad9f146e",
        ),
        (
            b"Hello-World",
            "df99966a707b8ed42c5d8cd1dc69a6e22517245be8b784b7a87705ac4c0835c5",
        ),
        (
            br#"{"hello":"world"}"#,
            "93a23971a914e5eacbf0a8d25154cda309c3c1c72fbb9914d47c60f3cb681588",
        ),
        (
            b"\x00\x01",
            "b413f47d13ee2fe6c845b2ee141af81de858df4ec549a58b7970bb96645bc8d2",
        ),
    ] {
        assert_eq!(BodyDigest::Raw(body).hex(), digest);
        assert_eq!(
            BodyDigest::Sha256(Sha256::digest(body).into()).hex(),
            digest
        );
    }
    let apex: Value = serde_json::from_str(r#"{"b":1,"a":"x"}"#).unwrap();
    let canonical = canonical_json(&apex);
    assert_eq!(canonical, br#"{"a": "x", "b": 1}"#);
    assert_eq!(
        sha_hex(&canonical),
        "385820f0096fd558f4091319e7fa742cebf877dc3baca180981889f1c40eca84"
    );
    let empty = canonical_json(&Value::Object(Default::default()));
    assert_eq!(
        sha_hex(&empty),
        "44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a"
    );
}

#[test]
fn request_messages_match_spec_table() {
    let json = sha_hex(JSON_BODY);
    let empty = sha_hex(b"");
    let hello = sha_hex(b"Hello World");
    let apex = sha_hex(br#"{"a": "x", "b": 1}"#);
    let apex_empty = sha_hex(b"{}");
    for (message, length, digest) in [
        (
            request_message(
                MessageForm::WithUuid,
                &json,
                Some(UUID),
                "1700000000123",
                BOB,
            ),
            164,
            "2ee2f3a0d9b4a2567f6427d8f084608c853939ca464488ef11a84ab14bd49deb",
        ),
        (
            request_message(
                MessageForm::WithUuid,
                &empty,
                Some(UUID),
                "1700000000123",
                "",
            ),
            116,
            "bbc8247ff0d0d1a327d8a1511190f63087cd683de3a98d2299bc0d1611f82098",
        ),
        (
            request_message(
                MessageForm::WithUuid,
                &hello,
                Some(UUID),
                "1700000000123",
                BOB,
            ),
            164,
            "1d3e2368f83b2d72d3b074d58c71de5a2b7eddf1a54c542b5a4a1cb3c63152d4",
        ),
        (
            request_message(MessageForm::NoUuid, &apex, Some(UUID), "1700000000123", ""),
            79,
            "02629c032fb6f2c66a3fba6d03c6a60171c32b580b9527b345d97dd9c215735c",
        ),
        (
            request_message(MessageForm::NoUuid, &apex, None, "1700000000123", BOB),
            127,
            "204b45dba6e6f0dc1f75ff2d94f53630d98f49321d0c873b9db7a836d2399465",
        ),
        (
            request_message(MessageForm::NoUuid, &apex_empty, None, "1700000000123", ""),
            79,
            "a89c2e690874a212d4973540c91721770d5e90f1ed8ca892161c626d49446b9d",
        ),
    ] {
        assert_eq!(message.len(), length, "{message}");
        assert_eq!(sha_hex(message.as_bytes()), digest, "{message}");
        assert!(!message.starts_with("0x"));
    }
}

#[test]
fn secret_messages_match_spec_table() {
    assert_eq!(interval(TS), (1_700_000_010_000, 170_000_001));
    let table = [
        (
            IntervalFormat::PyFloatMs,
            BOB,
            [
                "a0913815d86db96430997c7ec097747f11052942b79615dd5f54de4fb2869f71",
                "80654918417f300ecf3933e965a73b2ab4f0d1325102ef76cbe6383d03d2abfa",
                "777798514ce8abf5916be41db1897dd7f08a10d43897f9a9b38bb675980575ed",
            ],
            64,
        ),
        (
            IntervalFormat::IntMs,
            BOB,
            [
                "3ce4cebfc86f756a2a060296c5cba875908133d4437d0c2fe307ee82cef9e773",
                "eae51c9dcf5fcf039a48e93d1fffc1170a310e50c564997b11a8c2c5ebf0b3fe",
                "bdc454d1a28ba1f81507e78139e2440663369463aca7377bfdb7933de31aaff1",
            ],
            62,
        ),
        (
            IntervalFormat::Index,
            ALICE,
            [
                "45f0dc7c8cf8935b13f168ac08fa0656c8f2e0b6c4e1d24f68e8a21e04703e40",
                "4a76962c0eba99e573b8dc6f54049ec00b0bf4ccb5be72f1d48bb20d178ee03a",
                "d4090e12c3106e7258c46d5e9acaa323c04cf092bdddb6983b8d92c0bb2595a4",
            ],
            58,
        ),
        (
            IntervalFormat::Index,
            BOB,
            [
                "cd70dcc587b8130c0e6241a1c5e03f46bfa7403458a4b68c1d54794741717099",
                "3aa0b4d1d9e3b12f13c72b13da87066653fc33a7a92af698d2f7187f299b09f8",
                "939bf158b909ccc8a07b57cd83d6693d7baef7a72a2eaf7ddd1d1e863a8089cb",
            ],
            58,
        ),
    ];
    for (format, subject, digests, length) in table {
        for (k, digest) in (0u8..).zip(digests) {
            let message = secret_message(format, TS, k, subject).unwrap();
            assert_eq!(message.len(), length, "{message}");
            assert_eq!(sha_hex(message.as_bytes()), digest, "{message}");
        }
    }
    assert_eq!(
        secret_message(IntervalFormat::PyFloatMs, TS, 0, BOB).unwrap(),
        format!("1700000009999.0.{BOB}")
    );
}

#[tokio::test]
async fn signers_sign_the_spec_messages() -> TestResult {
    let signer = RecordingSigner::new();
    let m1 = request_message(
        MessageForm::WithUuid,
        &sha_hex(JSON_BODY),
        Some(UUID),
        "1700000000123",
        BOB,
    );
    for dialect in [Dialect::RTAO, Dialect::BITMIND, Dialect::SPEC] {
        let headers = sign_headers(&signer, &dialect, opts(JSON_BODY, Some(BOB))).await?;
        let messages = signer.take();
        assert_eq!(messages[0], m1);
        assert_eq!(
            &messages[1..],
            [0, 1, 2].map(|k| secret_message(IntervalFormat::PyFloatMs, TS, k, BOB).unwrap())
        );
        let names: Vec<_> = headers.iter().map(|(name, _)| name).collect();
        assert_eq!(
            names,
            [
                header::VERSION,
                header::TIMESTAMP,
                header::UUID,
                header::SIGNED_BY,
                header::SIGNED_FOR,
                header::REQUEST_SIGNATURE,
                header::SECRET_SIGNATURE[0],
                header::SECRET_SIGNATURE[1],
                header::SECRET_SIGNATURE[2],
            ]
        );
        assert_eq!(headers.timestamp, "1700000000123");
        assert_eq!(headers.signed_by, ALICE);
        assert_eq!(headers.version, Some("2"));
    }
    // Unbound spec signing (M2): one signature and no receiver headers.
    let headers = sign_headers(&signer, &Dialect::SPEC, opts(b"", None)).await?;
    assert_eq!(
        signer.take(),
        [format!("{}.{UUID}.1700000000123.", sha_hex(b""))]
    );
    assert!(headers.signed_for.is_none() && headers.secret.is_none());
    // gobt (M3): sender-subject interval-index secrets.
    sign_headers(&signer, &Dialect::GOBT, opts(b"Hello World", Some(BOB))).await?;
    let messages = signer.take();
    assert_eq!(
        sha_hex(messages[0].as_bytes()),
        "1d3e2368f83b2d72d3b074d58c71de5a2b7eddf1a54c542b5a4a1cb3c63152d4"
    );
    assert_eq!(
        &messages[1..],
        [0, 1, 2].map(|k| secret_message(IntervalFormat::Index, TS, k, ALICE).unwrap())
    );
    // Apex (M4, M5): no uuid, request id extra.
    let body = canonical_json(&serde_json::from_str(r#"{"b":1,"a":"x"}"#)?);
    let headers = sign_headers(&signer, &Dialect::APEX, opts(&body, None)).await?;
    assert_eq!(
        sha_hex(signer.take()[0].as_bytes()),
        "02629c032fb6f2c66a3fba6d03c6a60171c32b580b9527b345d97dd9c215735c"
    );
    assert_eq!(headers.version, None);
    assert_eq!(headers.uuid, None);
    assert_eq!(headers.apex, Some(("100000".into(), UUID.into())));
    let pairs: Vec<_> = headers.iter().collect();
    assert_eq!(pairs.last(), Some(&(header::APEX_REQUEST_ID, UUID)));
    sign_headers(&signer, &Dialect::APEX, opts(&body, Some(BOB))).await?;
    assert_eq!(
        sha_hex(signer.take()[0].as_bytes()),
        "204b45dba6e6f0dc1f75ff2d94f53630d98f49321d0c873b9db7a836d2399465"
    );
    Ok(())
}

#[tokio::test]
async fn signing_refuses_misuse() -> TestResult {
    let signer = alice();
    let usage = |result: Result<_, EpistulaError>| matches!(result, Err(EpistulaError::Usage(_)));
    assert!(usage(
        sign_headers(&signer, &Dialect::RTAO, opts(b"", None)).await
    ));
    assert!(usage(
        sign_headers(&signer, &Dialect::GOBT, opts(b"", None)).await
    ));
    assert!(usage(
        sign_headers(&signer, &Dialect::BITMIND, opts(b"", Some(""))).await
    ));
    let mut bad_uuid = opts(b"", Some(BOB));
    bad_uuid.uuid = Some("has space");
    assert!(usage(sign_headers(&signer, &Dialect::RTAO, bad_uuid).await));
    let ed = KeypairSigner::from_uri("//Alice", CryptoType::Ed25519)?;
    assert_eq!(
        sign_headers(&ed, &Dialect::RTAO, opts(b"", Some(BOB))).await,
        Err(EpistulaError::UnsupportedCrypto)
    );
    // Fresh uuids are lowercase version-4 values and differ per attempt.
    let first = sign_headers(
        &signer,
        &Dialect::RTAO,
        SignOptions::new(b"", TS).signed_for(BOB),
    )
    .await?;
    let second = sign_headers(
        &signer,
        &Dialect::RTAO,
        SignOptions::new(b"", TS).signed_for(BOB),
    )
    .await?;
    let uuid = first.uuid.clone().unwrap();
    assert_ne!(first.uuid, second.uuid);
    assert_eq!(uuid.len(), 36);
    assert_eq!(&uuid[14..15], "4");
    assert!(matches!(&uuid[19..20], "8" | "9" | "a" | "b"));
    assert_eq!(uuid, uuid.to_lowercase());
    Ok(())
}

fn verifier(dialect: Dialect) -> Verifier {
    Verifier::without_replay(bob(), dialect)
}

async fn signed_map(dialect: &Dialect, body: &[u8], signed_for: Option<&str>) -> HeaderMap {
    sign_headers(&alice(), dialect, opts(body, signed_for))
        .await
        .unwrap()
        .to_header_map()
        .unwrap()
}

fn name(name: &str) -> HeaderName {
    HeaderName::from_bytes(name.as_bytes()).unwrap()
}

struct FixedClock(u64);
impl rtao_btauth::Clock for FixedClock {
    fn now_ns(&self) -> Result<u64, rtao_btauth::ClockError> {
        Ok(self.0 * 1_000_000)
    }
}

fn set(map: &mut HeaderMap, header: &str, value: &str) {
    map.insert(name(header), HeaderValue::from_str(value).unwrap());
}

#[tokio::test]
async fn every_preset_round_trips() -> TestResult {
    for dialect in [
        Dialect::RTAO,
        Dialect::BITMIND,
        Dialect::SPEC,
        Dialect::GOBT,
    ] {
        let headers = signed_map(&dialect, JSON_BODY, Some(BOB)).await;
        let caller = verifier(dialect.clone())
            .verify(&headers, BodyDigest::Raw(JSON_BODY), &FixedClock(TS))
            .await?;
        assert_eq!(caller.signed_by, ALICE);
        assert_eq!(caller.hotkey, alice().account());
        assert_eq!(caller.timestamp_ms, i128::from(TS));
        assert_eq!(caller.uuid.as_deref(), Some(UUID));
        assert_eq!(caller.signed_for.as_deref(), Some(BOB));
    }
    let body = canonical_json(&serde_json::from_str(r#"{"b":1,"a":"x"}"#)?);
    let headers = signed_map(&Dialect::APEX, &body, None).await;
    let caller = verifier(Dialect::APEX)
        .verify(
            &headers,
            BodyDigest::Sha256(Sha256::digest(&body).into()),
            &FixedClock(TS),
        )
        .await?;
    assert_eq!((caller.uuid, caller.signed_for), (None, None));
    // M5: an Apex request bound to a receiver never verifies on Apex.
    let headers = signed_map(&Dialect::APEX, &body, Some(BOB)).await;
    assert_eq!(
        verifier(Dialect::APEX)
            .authenticate(&headers, BodyDigest::Raw(&body), TS)
            .err(),
        Some(EpistulaError::SignatureMismatch)
    );
    // M2 is unbound: RTAO refuses it, SPEC accepts it with an empty Signed-For.
    let mut unbound = signed_map(&Dialect::SPEC, b"", None).await;
    assert_eq!(
        verifier(Dialect::RTAO)
            .authenticate(&unbound, BodyDigest::Raw(b""), TS)
            .err(),
        Some(EpistulaError::NotForSelf)
    );
    assert_eq!(
        verifier(Dialect::SPEC)
            .authenticate(&unbound, BodyDigest::Raw(b""), TS)
            .err(),
        Some(EpistulaError::InvalidReceiverKey)
    );
    set(&mut unbound, header::SIGNED_FOR, "");
    assert!(
        verifier(Dialect::SPEC)
            .authenticate(&unbound, BodyDigest::Raw(b""), TS)
            .is_ok()
    );
    Ok(())
}

#[tokio::test]
async fn freshness_boundaries_and_strings() -> TestResult {
    let headers = signed_map(&Dialect::BITMIND, JSON_BODY, Some(BOB)).await;
    let check = |dialect: Dialect, now: u64| {
        verifier(dialect)
            .authenticate(&headers, BodyDigest::Raw(JSON_BODY), now)
            .err()
            .map(|error| error.to_string())
    };
    let stale = |text: &str| Some(text.to_owned());
    assert_eq!(check(Dialect::BITMIND, 1_700_000_015_123), None);
    assert_eq!(
        check(Dialect::BITMIND, 1_700_000_015_124),
        stale("Request is too stale: 15.0s old (limit: 15.0s)")
    );
    assert_eq!(
        check(Dialect::BITMIND, 1_700_000_020_124),
        stale("Request is too stale: 20.0s old (limit: 15.0s)")
    );
    assert_eq!(
        check(Dialect::BITMIND, TS + 15_250),
        stale("Request is too stale: 15.2s old (limit: 15.0s)")
    );
    assert_eq!(check(Dialect::BITMIND, 1_699_999_985_123), None);
    assert_eq!(
        check(Dialect::BITMIND, 1_699_999_985_122),
        stale("Request timestamp is in the future: 15.0s ahead (limit: 15.0s)")
    );
    assert_eq!(check(Dialect::RTAO, TS - 2_000), None);
    assert_eq!(
        check(Dialect::RTAO, TS - 2_001),
        stale("Request timestamp is in the future: 2.0s ahead (limit: 2.0s)")
    );
    assert_eq!(check(Dialect::SPEC, 1_700_000_008_123), None);
    assert_eq!(
        check(Dialect::SPEC, 1_700_000_008_124),
        stale("Request is too stale")
    );
    assert_eq!(check(Dialect::SPEC, 0), None);
    Ok(())
}

#[tokio::test]
async fn header_grammar_negatives() -> TestResult {
    let base = signed_map(&Dialect::RTAO, JSON_BODY, Some(BOB)).await;
    let rtao = verifier(Dialect::RTAO);
    let run = |headers: &HeaderMap| {
        rtao.authenticate(headers, BodyDigest::Raw(JSON_BODY), TS)
            .err()
    };

    let mut duplicate = base.clone();
    duplicate.append(name(header::UUID), HeaderValue::from_static(UUID));
    assert_eq!(run(&duplicate), Some(EpistulaError::InvalidUuid));
    for (header_name, error) in [
        (header::VERSION, EpistulaError::UnknownVersion),
        (header::TIMESTAMP, EpistulaError::InvalidTimestamp),
        (header::SIGNED_BY, EpistulaError::InvalidSenderKey),
        (header::SIGNED_FOR, EpistulaError::InvalidReceiverKey),
        (header::REQUEST_SIGNATURE, EpistulaError::InvalidSignature),
    ] {
        let mut duplicate = base.clone();
        let value = duplicate.get(header_name).unwrap().clone();
        duplicate.append(name(header_name), value);
        assert_eq!(run(&duplicate), Some(error), "{header_name}");
    }
    let mut long_uuid = base.clone();
    set(&mut long_uuid, header::UUID, &"a".repeat(129));
    assert_eq!(run(&long_uuid), Some(EpistulaError::InvalidUuid));
    let mut long_sender = base.clone();
    set(&mut long_sender, header::SIGNED_BY, &"5".repeat(50));
    assert_eq!(run(&long_sender), Some(EpistulaError::InvalidSenderKey));
    let mut bad_sender = base.clone();
    set(&mut bad_sender, header::SIGNED_BY, "not-an-address");
    assert_eq!(run(&bad_sender), Some(EpistulaError::InvalidSenderKey));
    let mut huge = base.clone();
    set(&mut huge, header::UUID, &"a".repeat(4_097));
    assert_eq!(run(&huge), Some(EpistulaError::HeaderBudget));
    // Each field fits 4,096 bytes, but together they exceed 8,192.
    let mut total = base.clone();
    for header_name in [header::UUID, header::SIGNED_BY, header::SIGNED_FOR] {
        set(&mut total, header_name, &"a".repeat(3_000));
    }
    assert_eq!(run(&total), Some(EpistulaError::HeaderBudget));
    let mut within = base.clone();
    for header_name in [header::UUID, header::SIGNED_FOR] {
        set(&mut within, header_name, &"a".repeat(3_000));
    }
    assert_ne!(run(&within), Some(EpistulaError::HeaderBudget));
    let mut prefix0 = base.clone();
    set(&mut prefix0, header::SIGNED_FOR, BOB_PREFIX0);
    assert_eq!(run(&prefix0), Some(EpistulaError::NotForSelf));
    let mut non_utf8 = base.clone();
    non_utf8.insert(
        name(header::UUID),
        HeaderValue::from_bytes(b"\xff\xfe").unwrap(),
    );
    assert_eq!(run(&non_utf8), Some(EpistulaError::InvalidUuid));
    // gobt ignores the version and Signed-For, using its own address.
    let mut gobt = signed_map(&Dialect::GOBT, JSON_BODY, Some(BOB)).await;
    gobt.remove(header::VERSION);
    gobt.remove(header::SIGNED_FOR);
    assert!(
        verifier(Dialect::GOBT)
            .authenticate(&gobt, BodyDigest::Raw(JSON_BODY), TS)
            .is_ok()
    );
    Ok(())
}

#[tokio::test]
async fn replay_store_rejects_reuse_and_enforces_retention() -> TestResult {
    let store = Arc::new(InMemoryNonceStore::new());
    let verifier = Verifier::new(bob(), Dialect::RTAO, store.clone())?;
    let headers = signed_map(&Dialect::RTAO, JSON_BODY, Some(BOB)).await;
    verifier
        .verify(&headers, BodyDigest::Raw(JSON_BODY), &FixedClock(TS))
        .await?;
    assert_eq!(
        verifier
            .verify(&headers, BodyDigest::Raw(JSON_BODY), &FixedClock(TS))
            .await,
        Err(EpistulaError::Replayed)
    );
    // Same uuid, fresh signature: still a replay.
    let again = signed_map(&Dialect::RTAO, JSON_BODY, Some(BOB)).await;
    assert_ne!(
        again.get(header::REQUEST_SIGNATURE),
        headers.get(header::REQUEST_SIGNATURE)
    );
    assert_eq!(
        verifier
            .verify(&again, BodyDigest::Raw(JSON_BODY), &FixedClock(TS))
            .await,
        Err(EpistulaError::Replayed)
    );
    // Apex has no uuid; the signature is the identity.
    let apex_store = Arc::new(InMemoryNonceStore::new());
    let mut apex = Dialect::APEX;
    apex.max_future_ms = Some(2_000);
    let apex_verifier = Verifier::new(bob(), apex.clone(), apex_store)?;
    let headers = sign_headers(&alice(), &apex, opts(b"{}", None))
        .await?
        .to_header_map()?;
    apex_verifier
        .verify(&headers, BodyDigest::Raw(b"{}"), &FixedClock(TS))
        .await?;
    assert_eq!(
        apex_verifier
            .verify(&headers, BodyDigest::Raw(b"{}"), &FixedClock(TS))
            .await,
        Err(EpistulaError::Replayed)
    );

    let store_for = |seconds: u64| {
        Arc::new(
            InMemoryNonceStore::with_config(NonceStoreConfig {
                retention: Duration::from_secs(seconds),
                ..NonceStoreConfig::default()
            })
            .unwrap(),
        )
    };
    assert!(Verifier::new(bob(), Dialect::BITMIND, store_for(30)).is_err());
    assert!(Verifier::new(bob(), Dialect::BITMIND, store_for(31)).is_ok());
    assert!(Verifier::new(bob(), Dialect::RTAO, store_for(17)).is_err());
    assert!(Verifier::new(bob(), Dialect::RTAO, store_for(18)).is_ok());
    assert!(Verifier::new(bob(), Dialect::SPEC, store_for(600)).is_err());
    assert!(Verifier::new(bob(), Dialect::APEX, store_for(600)).is_err());
    let disabled =
        Verifier::with_store(bob(), Dialect::SPEC, store_for(1), ReplayPolicy::Disabled)?;
    let headers = signed_map(&Dialect::SPEC, JSON_BODY, Some(BOB)).await;
    disabled
        .verify(&headers, BodyDigest::Raw(JSON_BODY), &FixedClock(TS))
        .await?;
    disabled
        .verify(&headers, BodyDigest::Raw(JSON_BODY), &FixedClock(TS))
        .await?;
    Ok(())
}

#[tokio::test]
async fn commit_bounds_the_gap_after_a_head_time_check() -> TestResult {
    // 60 s retention and a 17 s RTAO window leave less than 43 s for a commit.
    let store = Arc::new(InMemoryNonceStore::new());
    let enforcing = Verifier::new(bob(), Dialect::RTAO, store.clone())?;
    assert_eq!(
        enforcing.commit_slack(),
        Some(Duration::from_millis(42_999))
    );
    let headers = signed_map(&Dialect::RTAO, JSON_BODY, Some(BOB)).await;
    // Freshness checked when the head arrived, at the edge of the age window.
    let head = TS + 15_000;
    let late = enforcing.authenticate(&headers, BodyDigest::Raw(JSON_BODY), head)?;
    assert_eq!(late.checked_at_ms(), head);
    assert_eq!(
        late.commit(store.as_ref(), &FixedClock(head + 43_000))
            .await,
        Err(EpistulaError::DeadlineExceeded)
    );
    let in_time = enforcing.authenticate(&headers, BodyDigest::Raw(JSON_BODY), head)?;
    assert!(
        in_time
            .commit(store.as_ref(), &FixedClock(head + 42_999))
            .await
            .is_ok()
    );
    // A dialect with unbounded skew cannot commit to a finite store.
    let spec = verifier(Dialect::SPEC).authenticate(
        &signed_map(&Dialect::SPEC, JSON_BODY, Some(BOB)).await,
        BodyDigest::Raw(JSON_BODY),
        TS,
    )?;
    assert!(matches!(
        spec.commit(store.as_ref(), &FixedClock(TS)).await,
        Err(EpistulaError::Usage(_))
    ));
    // check_head refuses stale heads before any signature check.
    let mut forged = headers.clone();
    set(
        &mut forged,
        header::REQUEST_SIGNATURE,
        &format!("0x{}", "00".repeat(64)),
    );
    assert_eq!(enforcing.check_head(&forged, TS)?, alice().account());
    assert!(matches!(
        enforcing.check_head(&forged, TS + 15_001),
        Err(EpistulaError::Stale(_))
    ));
    Ok(())
}

#[tokio::test]
async fn apply_replaces_every_epistula_header() -> TestResult {
    let mut map = signed_map(&Dialect::RTAO, JSON_BODY, Some(BOB)).await;
    assert!(map.contains_key(header::SIGNED_FOR));
    set(&mut map, "Epistula-Extra", "stale");
    set(&mut map, "X-Other", "kept");
    let unbound = sign_headers(&alice(), &Dialect::SPEC, opts(JSON_BODY, None)).await?;
    unbound.apply(&mut map)?;
    assert!(!map.contains_key(header::SIGNED_FOR));
    assert!(!map.contains_key("epistula-extra"));
    for secret in header::SECRET_SIGNATURE {
        assert!(!map.contains_key(secret));
    }
    assert_eq!(map["x-other"], "kept");
    let epistula = map
        .keys()
        .filter(|name| name.as_str().starts_with("epistula-"))
        .count();
    assert_eq!(epistula, unbound.iter().count());
    Ok(())
}

#[tokio::test]
async fn btauth_caller_adapter() -> TestResult {
    let headers = signed_map(&Dialect::RTAO, JSON_BODY, Some(BOB)).await;
    let authenticated =
        verifier(Dialect::RTAO).authenticate(&headers, BodyDigest::Raw(JSON_BODY), TS)?;
    let caller = authenticated.btauth_caller();
    assert_eq!(caller.hotkey, alice().account());
    assert_eq!(caller.nonce_ns, num_bigint::BigInt::from(TS) * 1_000_000);
    assert_eq!(caller.crypto, CryptoType::Sr25519);
    Ok(())
}

// ---------------------------------------------------------------------------
// Python reference parity.

#[derive(Debug, Clone, PartialEq, Eq)]
enum Verdict {
    Ok,
    Err(String),
}

fn python_verdict(value: &Value) -> Option<Verdict> {
    match value["status"].as_u64()? {
        200 => Some(Verdict::Ok),
        400 => Some(Verdict::Err(value["detail"].as_str()?.to_owned())),
        // An uncaught Python exception (HTTP 500); rtao answers 400 (D-E4).
        _ => None,
    }
}

fn rtao_verdict(
    dialect: Dialect,
    me: &AccountId,
    headers: &HeaderMap,
    body: &[u8],
    now: u64,
) -> Verdict {
    match Verifier::without_replay(*me, dialect).authenticate(headers, BodyDigest::Raw(body), now) {
        Ok(_) => Verdict::Ok,
        Err(error) => Verdict::Err(error.to_string()),
    }
}

fn err(text: &str) -> Option<Verdict> {
    Some(Verdict::Err(text.to_owned()))
}

/// Deliberate deviations from a Python verdict, by `(preset, vector)`.
fn deviation(preset: &str, id: &str) -> Option<Verdict> {
    const INVALID_SIGNATURE: &str = "Invalid Signature";
    match (preset, id) {
        // D-E4: malformed signatures raise in btwallet (HTTP 500).
        (_, "m1-sig-no-prefix" | "m1-sig-short") => err(INVALID_SIGNATURE),
        // D-E6: btwallet strips repeated 0x prefixes.
        (_, "m1-sig-double-prefix") => err(INVALID_SIGNATURE),
        // D-E4: the spec verifier's int() raises (HTTP 500).
        ("spec", "m1-ts-float" | "m1-ts-absent") => err("Invalid Timestamp"),
        // Apex: a missing signature fails pydantic validation (HTTP 500), and a
        // missing timestamp defaults to the receiver's seconds clock.
        ("apex", "m1-sig-absent") => err(INVALID_SIGNATURE),
        ("apex", "m1-ts-absent") => err("Invalid Timestamp"),
        // RTAO: canonical timestamps only (Q4) and a 2 s future bound (D-E5).
        ("rtao", "m1-ts-plus" | "m1-ts-leading-zero" | "m1-ts-underscore") => {
            err("Invalid Timestamp")
        }
        ("rtao", "m1-now-future") => {
            err("Request timestamp is in the future: 15.0s ahead (limit: 2.0s)")
        }
        ("rtao", "m1-now-future-equal") => {
            err("Request timestamp is in the future: 15.0s ahead (limit: 2.0s)")
        }
        ("rtao", "m1-now-future-2001") => {
            err("Request timestamp is in the future: 2.0s ahead (limit: 2.0s)")
        }
        // gobt: receiver = self (header ignored), version ignored, raw Go
        // ParseInt timestamps, 2 s future bound.
        ("gobt", "bm-json-unbound") => err("Signature Mismatch"),
        ("gobt", "m1-for-bob-prefix0" | "m1-for-charlie" | "m1-for-absent" | "m1-for-empty") => {
            Some(Verdict::Ok)
        }
        ("gobt", "m1-version-1" | "m1-version-absent") => Some(Verdict::Ok),
        ("gobt", "m1-ts-plus" | "m1-ts-leading-zero") => err("Signature Mismatch"),
        ("gobt", "m1-ts-underscore") => err("Invalid Timestamp"),
        ("gobt", "apex-unbound" | "apex-empty" | "apex-bound") => err("Invalid uuid"),
        ("gobt", "m1-now-future") => {
            err("Request timestamp is in the future: 15.0s ahead (limit: 2.0s)")
        }
        ("gobt", "m1-now-future-equal") => {
            err("Request timestamp is in the future: 15.0s ahead (limit: 2.0s)")
        }
        ("gobt", "m1-now-future-2001") => {
            err("Request timestamp is in the future: 2.0s ahead (limit: 2.0s)")
        }
        _ => None,
    }
}

fn python_vectors() -> TestResult<Value> {
    let path = std::env::var_os("EPISTULA_PY_VECTORS").map_or_else(
        || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/vectors/python.json"),
        PathBuf::from,
    );
    Ok(serde_json::from_slice(&std::fs::read(path)?)?)
}

#[test]
fn python_reference_parity() -> TestResult {
    let data = python_vectors()?;
    let me: AccountId = data["me"].as_str().unwrap().parse()?;
    assert_eq!(me, bob());
    let mut checked = 0;
    let mut failures = Vec::new();
    for vector in data["vectors"].as_array().unwrap() {
        let id = vector["id"].as_str().unwrap();
        let body = hex::decode(vector["body_hex"].as_str().unwrap())?;
        let now = vector["now_ms"].as_u64().unwrap();
        let mut headers = HeaderMap::new();
        for pair in vector["headers"].as_array().unwrap() {
            headers.append(
                HeaderName::from_bytes(pair[0].as_str().unwrap().as_bytes())?,
                HeaderValue::from_str(pair[1].as_str().unwrap())?,
            );
        }
        // BitMind's verdict is the reference for RTAO and gobt too, with the
        // deliberate differences listed in `deviation`. For gobt this is a
        // self-derived expectation, not a Go oracle.
        for (preset, dialect, reference) in [
            ("bitmind", Dialect::BITMIND, "bitmind"),
            ("rtao", Dialect::RTAO, "bitmind"),
            ("gobt", Dialect::GOBT, "bitmind"),
            ("spec", Dialect::SPEC, "spec"),
            ("apex", Dialect::APEX, "apex"),
        ] {
            // The Apex dialect has no uuid or version; compare it only on
            // Apex fixtures and M1 mutations, where its message is defined.
            if preset == "gobt" && id.starts_with("apex") && deviation(preset, id).is_none() {
                continue;
            }
            let expected = deviation(preset, id)
                .or_else(|| python_verdict(&vector[reference]))
                .unwrap_or_else(|| panic!("{preset}/{id}: Python raised; add a deviation"));
            let got = rtao_verdict(dialect, &me, &headers, &body, now);
            checked += 1;
            if got != expected {
                failures.push(format!("{preset}/{id}: expected {expected:?}, got {got:?}"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    assert!(checked >= 150, "only {checked} checks");
    Ok(())
}

#[test]
fn wrapped_fixture_follows_policy() -> TestResult {
    let data = python_vectors()?;
    let vector = data["vectors"]
        .as_array()
        .unwrap()
        .iter()
        .find(|vector| vector["id"] == "bm-json-wrapped")
        .unwrap();
    let mut headers = HeaderMap::new();
    for pair in vector["headers"].as_array().unwrap() {
        headers.append(
            HeaderName::from_bytes(pair[0].as_str().unwrap().as_bytes())?,
            HeaderValue::from_str(pair[1].as_str().unwrap())?,
        );
    }
    let mut strict = Dialect::RTAO;
    strict.wrapped = WrappedPolicy::Strict;
    assert!(
        verifier(Dialect::RTAO)
            .authenticate(&headers, BodyDigest::Raw(JSON_BODY), TS)
            .is_ok()
    );
    assert_eq!(
        verifier(strict)
            .authenticate(&headers, BodyDigest::Raw(JSON_BODY), TS)
            .err(),
        Some(EpistulaError::SignatureMismatch)
    );
    Ok(())
}
