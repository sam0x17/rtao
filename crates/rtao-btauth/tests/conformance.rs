// Derived from RaoFoundation/subtensor sdk/python/tests/unit/test_http_auth.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Cross-language btauth/1 vectors and authentication-order invariants.

use std::{
    error::Error,
    io,
    sync::{
        Arc, Barrier,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
};

use futures::executor::block_on;
use http::{HeaderMap, HeaderValue, Method, header::HeaderName};
use num_bigint::BigInt;
use rtao_btauth::{
    AuthError, BoxFuture, Caller, Clock, ClockError, ConfigError, DEFAULT_ALLOWED_SKEW,
    DEFAULT_MAX_AGE, HEADER_CRYPTO, HEADER_HOTKEY, HEADER_NONCE, HEADER_RECEIVER, HEADER_SIGNATURE,
    HEADER_VERSION, InMemoryNonceStore, NonceStore, NonceStoreConfig, PROTOCOL, StoreError,
    SystemClock, VERSION, Verifier, VerifyConfig, WrappedPolicy, build_payload, sign,
};
use rtao_primitives::{AccountId, CryptoType};
use rtao_signer::{KeypairSigner, MultiSignature, Signer, SignerError, SigningRequest};
use serde_json::Value;
use sha2::{Digest, Sha256};

type TestResult = Result<(), Box<dyn Error>>;

const FIXED_NONCE: u64 = 1_752_076_800_000_000_000;
const NOW: u64 = FIXED_NONCE + 1_000_000;
const BODY: &[u8] = b"{\"prompt\": \"hello\"}";
const TARGET: &str = "/generate?stream=false";
const ALGORITHMS: [CryptoType; 2] = [CryptoType::Sr25519, CryptoType::Ed25519];
const ALICE_SR: &str = "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY";
const ALICE_ED: &str = "5FA9nQDVg267DEd8m1ZypXLBnvN7SFxYwV7ndqSYGiN9TTpu";
const BOB: &str = "5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty";
const BAD_SIGNATURE: &str = "signature does not verify against the claimed hotkey";
const INVALID_ENCODING: &str = "invalid hotkey address or signature encoding";
const REPLAY: &str = "this nonce was already accepted from this hotkey";
const MISSING_RECEIVER: &str =
    "missing X-Bittensor-Receiver: this server requires receiver-bound requests";
const WRONG_RECEIVER: &str = "request was signed for a different receiving hotkey";

struct FixedClock(u64);

impl Clock for FixedClock {
    fn now_ns(&self) -> Result<u64, ClockError> {
        Ok(self.0)
    }
}

fn alice(crypto: CryptoType) -> Result<KeypairSigner, SignerError> {
    KeypairSigner::from_uri("//Alice", crypto)
}

fn bob() -> Result<AccountId, SignerError> {
    Ok(KeypairSigner::from_uri("//Bob", CryptoType::Sr25519)?.account())
}

fn headers(sender: &dyn Signer, receiver: Option<&AccountId>) -> Result<HeaderMap, AuthError> {
    block_on(sign(
        sender,
        &Method::POST,
        TARGET,
        BODY,
        receiver,
        &FixedClock(FIXED_NONCE),
    ))
}

fn verifier(
    receiver: AccountId,
    cfg: VerifyConfig,
) -> Result<Verifier<InMemoryNonceStore>, ConfigError> {
    Verifier::new(receiver, cfg, Arc::new(InMemoryNonceStore::new()))
}

fn check(headers: &HeaderMap, receiver: AccountId) -> Result<Caller, Box<dyn Error>> {
    Ok(block_on(
        verifier(receiver, VerifyConfig::default())?.verify(
            headers,
            &Method::POST,
            TARGET,
            BODY,
            NOW,
        ),
    )?)
}

fn header_text<'a>(headers: &'a HeaderMap, name: &str) -> Result<&'a str, Box<dyn Error>> {
    Ok(headers
        .get(name)
        .ok_or_else(|| io::Error::other(format!("missing test header {name}")))?
        .to_str()?)
}

fn replace(headers: &mut HeaderMap, name: &str, value: &str) -> TestResult {
    headers.insert(
        HeaderName::from_bytes(name.as_bytes())?,
        HeaderValue::from_str(value)?,
    );
    Ok(())
}

fn assert_error<T>(result: Result<T, AuthError>, expected: AuthError) -> TestResult {
    let error = result
        .err()
        .ok_or_else(|| io::Error::other(format!("expected authentication error: {expected}")))?;
    assert_eq!(error.to_string(), expected.to_string());
    assert_eq!(error, expected);
    let _: &dyn Error = &error;
    Ok(())
}

fn fixture() -> Result<Value, serde_json::Error> {
    rtao_primitives::json::from_str(include_str!(
        "../../../vendor/upstream/extracted/btauth.json"
    ))
}

fn fixture_str<'a>(fixture: &'a Value, pointer: &str) -> Result<&'a str, Box<dyn Error>> {
    fixture
        .pointer(pointer)
        .and_then(Value::as_str)
        .ok_or_else(|| io::Error::other(format!("missing string fixture {pointer}")).into())
}

// pyt: unit/test_http_auth.py::test_golden_payload_bytes
#[test]
fn test_golden_payload_bytes() -> TestResult {
    let fixture = fixture()?;
    assert_eq!(fixture.pointer("/format").and_then(Value::as_u64), Some(1));
    assert_eq!(
        fixture
            .pointer("/values/FIXED_NONCE/value")
            .and_then(Value::as_u64),
        Some(FIXED_NONCE)
    );
    assert_eq!(fixture_str(&fixture, "/values/METHOD/value")?, "POST");
    assert_eq!(fixture_str(&fixture, "/values/PATH/value")?, TARGET);
    assert_eq!(
        hex::decode(fixture_str(&fixture, "/values/BODY/value/bytes_hex")?)?,
        BODY
    );
    let sender = alice(CryptoType::Sr25519)?;
    let receiver = bob()?;
    assert_eq!(sender.account().to_string(), ALICE_SR);
    assert_eq!(receiver.to_string(), BOB);
    let payload = build_payload(
        CryptoType::Sr25519,
        &Method::POST,
        TARGET,
        BODY,
        FIXED_NONCE,
        &sender.account(),
        Some(&receiver),
    );
    let golden = hex::decode(fixture_str(
        &fixture,
        "/values/GOLDEN_SR25519_PAYLOAD/value/bytes_hex",
    )?)?;
    assert_eq!(payload, golden);
    let text = std::str::from_utf8(&payload)?;
    assert_eq!(text.lines().count(), 8);
    assert!(!payload.ends_with(b"\n"));
    assert_eq!(text.lines().next(), Some(PROTOCOL));
    assert_eq!(
        text.lines().nth(4),
        Some(hex::encode(Sha256::digest(BODY)).as_str())
    );
    Ok(())
}

// pyt: unit/test_http_auth.py::test_golden_ed25519_signature
#[test]
fn test_golden_ed25519_signature() -> TestResult {
    let fixture = fixture()?;
    let sender = alice(CryptoType::Ed25519)?;
    let receiver = bob()?;
    let headers = headers(&sender, Some(&receiver))?;
    let golden = fixture_str(&fixture, "/values/GOLDEN_ED25519_SIGNATURE/value")?;
    assert_eq!(header_text(&headers, HEADER_SIGNATURE)?, golden);
    assert_eq!(header_text(&headers, HEADER_CRYPTO)?, "ed25519");
    assert_eq!(sender.account().to_string(), ALICE_ED);
    let payload = build_payload(
        CryptoType::Ed25519,
        &Method::POST,
        TARGET,
        BODY,
        FIXED_NONCE,
        &sender.account(),
        Some(&receiver),
    );
    let signature = hex::decode(
        golden
            .strip_prefix("0x")
            .ok_or_else(|| io::Error::other("golden signature must have a 0x prefix"))?,
    )?;
    assert_eq!(signature.len(), 64);
    assert!(rtao_core::keys::verify_strict(
        &payload, &signature, ALICE_ED, 0
    )?);
    assert!(!rtao_core::keys::verify_strict(
        &[b"<Bytes>".as_slice(), &payload, b"</Bytes>"].concat(),
        &signature,
        ALICE_ED,
        0,
    )?);
    assert_eq!(check(&headers, receiver)?.hotkey, sender.account());
    Ok(())
}

// pyt: unit/test_http_auth.py::test_golden_addresses
#[test]
fn test_golden_addresses() -> TestResult {
    assert_eq!(alice(CryptoType::Sr25519)?.account().to_string(), ALICE_SR);
    assert_eq!(alice(CryptoType::Ed25519)?.account().to_string(), ALICE_ED);
    assert_eq!(bob()?.to_string(), BOB);
    Ok(())
}

// pyt: unit/test_http_auth.py::test_sr25519_crypto_header_omitted
#[test]
fn test_sr25519_crypto_header_omitted() -> TestResult {
    let sender = alice(CryptoType::Sr25519)?;
    let receiver = bob()?;
    for receiver in [None, Some(&receiver)] {
        assert!(!headers(&sender, receiver)?.contains_key(HEADER_CRYPTO));
    }
    Ok(())
}

// pyt: unit/test_http_auth.py::test_round_trip
#[test]
fn test_round_trip() -> TestResult {
    let receiver = bob()?;
    for crypto in ALGORITHMS {
        let sender = alice(crypto)?;
        let headers = headers(&sender, Some(&receiver))?;
        assert_eq!(
            check(&headers, receiver)?,
            Caller {
                hotkey: sender.account(),
                nonce_ns: BigInt::from(FIXED_NONCE),
                crypto
            }
        );
        assert_eq!(header_text(&headers, HEADER_VERSION)?, VERSION);
        assert_eq!(
            header_text(&headers, HEADER_NONCE)?,
            FIXED_NONCE.to_string()
        );
        assert_eq!(
            header_text(&headers, HEADER_RECEIVER)?,
            receiver.to_string()
        );
        assert_eq!(
            header_text(&headers, HEADER_HOTKEY)?,
            sender.account().to_string()
        );
        let mut actual: Vec<_> = headers.keys().map(HeaderName::as_str).collect();
        actual.sort_unstable();
        let mut expected = vec![
            "x-bittensor-version",
            "x-bittensor-hotkey",
            "x-bittensor-nonce",
            "x-bittensor-receiver",
            "x-bittensor-signature",
        ];
        if crypto == CryptoType::Ed25519 {
            expected.push("x-bittensor-crypto");
        } else {
            assert!(!headers.contains_key(HEADER_CRYPTO));
        }
        expected.sort_unstable();
        assert_eq!(actual, expected);
        assert!(headers.keys().all(|name| !name.as_str().contains("drand")));
    }
    Ok(())
}

// pyt: unit/test_http_auth.py::test_round_trip_live_nonce_get_empty_body
#[test]
fn test_round_trip_live_nonce_get_empty_body() -> TestResult {
    let receiver = bob()?;
    let clock = SystemClock::default();
    for crypto in ALGORITHMS {
        let sender = alice(crypto)?;
        let headers = block_on(sign(
            &sender,
            &Method::GET,
            "/health",
            b"",
            Some(&receiver),
            &clock,
        ))?;
        let caller = block_on(verifier(receiver, VerifyConfig::default())?.verify(
            &headers,
            &Method::GET,
            "/health",
            b"",
            clock.now_ns()?,
        ))?;
        assert_eq!(caller.hotkey, sender.account());
        assert_eq!(caller.crypto, crypto);
        assert_eq!(
            caller.nonce_ns.to_string(),
            header_text(&headers, HEADER_NONCE)?
        );
    }
    Ok(())
}

#[test]
fn retry_signing_obtains_a_fresh_monotonic_nonce() -> TestResult {
    let sender = alice(CryptoType::Ed25519)?;
    let receiver = bob()?;
    let clock = SystemClock::default();
    let first = block_on(sign(
        &sender,
        &Method::POST,
        TARGET,
        BODY,
        Some(&receiver),
        &clock,
    ))?;
    let second = block_on(sign(
        &sender,
        &Method::POST,
        TARGET,
        BODY,
        Some(&receiver),
        &clock,
    ))?;
    let first_nonce: u64 = header_text(&first, HEADER_NONCE)?.parse()?;
    let second_nonce: u64 = header_text(&second, HEADER_NONCE)?.parse()?;
    assert!(second_nonce > first_nonce);
    assert_ne!(
        header_text(&first, HEADER_SIGNATURE)?,
        header_text(&second, HEADER_SIGNATURE)?
    );
    let verifier = verifier(receiver, VerifyConfig::default())?;
    let now = clock.now_ns()?;
    assert_eq!(
        block_on(verifier.verify(&first, &Method::POST, TARGET, BODY, now))?.nonce_ns,
        BigInt::from(first_nonce)
    );
    assert_eq!(
        block_on(verifier.verify(&second, &Method::POST, TARGET, BODY, now))?.nonce_ns,
        BigInt::from(second_nonce)
    );
    Ok(())
}

// pyt: unit/test_http_auth.py::test_headers_survive_lowercasing
#[test]
fn test_headers_survive_lowercasing() -> TestResult {
    let sender = alice(CryptoType::Sr25519)?;
    let receiver = bob()?;
    let original = headers(&sender, Some(&receiver))?;
    let mut lowered = HeaderMap::new();
    for (name, value) in &original {
        lowered.insert(
            HeaderName::from_bytes(name.as_str().to_lowercase().as_bytes())?,
            value.clone(),
        );
    }
    assert_eq!(check(&lowered, receiver)?.hotkey, sender.account());
    Ok(())
}

// pyt: unit/test_http_auth.py::test_non_canonical_scheme_spellings
#[test]
fn test_non_canonical_scheme_spellings() -> TestResult {
    let receiver = bob()?;
    for (crypto, aliases) in [
        (CryptoType::Sr25519, ["SR25519", "1", "  sR25519  ", "0001"]),
        (CryptoType::Ed25519, ["ED25519", "0", "  Ed25519  ", "0000"]),
    ] {
        let sender = alice(crypto)?;
        for alias in aliases {
            let mut headers = headers(&sender, Some(&receiver))?;
            replace(&mut headers, HEADER_CRYPTO, alias)?;
            let caller = check(&headers, receiver)?;
            assert_eq!(caller.hotkey, sender.account());
            assert_eq!(caller.crypto, crypto);
        }
    }
    Ok(())
}

#[test]
fn request_target_preserves_encoding_query_order_and_raw_body_bytes() -> TestResult {
    let receiver = bob()?;
    let target = "/generate/a%2Fb/%E2%98%83?stream=false&value=a%20b&value=%2B";
    let raw_body = b"{\n  \"prompt\" : \"hello\", \"bytes\": [0, 255]\n}\n\0\xff";
    for crypto in ALGORITHMS {
        let sender = alice(crypto)?;
        let headers = block_on(sign(
            &sender,
            &Method::POST,
            target,
            raw_body,
            Some(&receiver),
            &FixedClock(FIXED_NONCE),
        ))?;
        let verifier = verifier(receiver, VerifyConfig::default())?;
        assert_eq!(
            block_on(verifier.verify(&headers, &Method::POST, target, raw_body, NOW))?.hotkey,
            sender.account()
        );
        for changed_target in [
            "/generate/a/b/%E2%98%83?stream=false&value=a%20b&value=%2B",
            "/generate/a%2fb/%E2%98%83?stream=false&value=a%20b&value=%2B",
            "/generate/a%2Fb/%E2%98%83?stream=false&value=a+b&value=%2B",
            "/generate/a%2Fb/%E2%98%83?value=a%20b&stream=false&value=%2B",
            "/generate/a%2Fb/%E2%98%83",
        ] {
            assert_error(
                verifier.authenticate(&headers, &Method::POST, changed_target, raw_body, NOW),
                AuthError::BadSignature(BAD_SIGNATURE.into()),
            )?;
        }
        assert_error(
            verifier.authenticate(
                &headers,
                &Method::POST,
                target,
                b"{\"prompt\":\"hello\",\"bytes\":[0,255]}",
                NOW,
            ),
            AuthError::BadSignature(BAD_SIGNATURE.into()),
        )?;
    }
    Ok(())
}

#[test]
fn method_normalization_signs_uppercase_and_accepts_lowercase_at_verification() -> TestResult {
    let receiver = bob()?;
    let lower = Method::from_bytes(b"post")?;
    for crypto in ALGORITHMS {
        let sender = alice(crypto)?;
        assert_eq!(
            build_payload(
                crypto,
                &lower,
                TARGET,
                BODY,
                FIXED_NONCE,
                &sender.account(),
                Some(&receiver)
            ),
            build_payload(
                crypto,
                &Method::POST,
                TARGET,
                BODY,
                FIXED_NONCE,
                &sender.account(),
                Some(&receiver)
            ),
        );
        let headers = block_on(sign(
            &sender,
            &lower,
            TARGET,
            BODY,
            Some(&receiver),
            &FixedClock(FIXED_NONCE),
        ))?;
        assert_eq!(check(&headers, receiver)?.hotkey, sender.account());
        assert_eq!(
            verifier(receiver, VerifyConfig::default())?
                .authenticate(&headers, &lower, TARGET, BODY, NOW)?
                .caller()
                .hotkey,
            sender.account()
        );
    }
    Ok(())
}

// pyt: unit/test_http_auth.py::test_stale_and_future_nonces
#[test]
fn test_stale_and_future_nonces() -> TestResult {
    let sender = alice(CryptoType::Sr25519)?;
    let receiver = bob()?;
    let headers = headers(&sender, Some(&receiver))?;
    let verifier = verifier(receiver, VerifyConfig::default())?;
    for now in [FIXED_NONCE + 11_000_000_000, FIXED_NONCE + 10_000_000_001] {
        assert_error(
            verifier.authenticate(&headers, &Method::POST, TARGET, BODY, now),
            AuthError::StaleRequest("nonce is older than the 10s freshness window".into()),
        )?;
    }
    for now in [FIXED_NONCE - 3_000_000_000, FIXED_NONCE - 2_000_000_001] {
        assert_error(
            verifier.authenticate(&headers, &Method::POST, TARGET, BODY, now),
            AuthError::StaleRequest("nonce is more than 2s in the future".into()),
        )?;
    }
    for now in [FIXED_NONCE + 10_000_000_000, FIXED_NONCE - 2_000_000_000] {
        assert_eq!(
            verifier
                .authenticate(&headers, &Method::POST, TARGET, BODY, now)?
                .caller()
                .nonce_ns,
            BigInt::from(FIXED_NONCE)
        );
    }
    Ok(())
}

#[test]
fn freshness_uses_full_uint64_nanoseconds_without_signed_overflow() -> TestResult {
    let receiver = bob()?;
    for crypto in ALGORITHMS {
        let sender = alice(crypto)?;
        for nonce in [
            0,
            1,
            i64::MAX as u64,
            (i64::MAX as u64) + 1,
            u64::MAX - 1,
            u64::MAX,
        ] {
            let headers = block_on(sign(
                &sender,
                &Method::POST,
                TARGET,
                BODY,
                Some(&receiver),
                &FixedClock(nonce),
            ))?;
            assert_eq!(header_text(&headers, HEADER_NONCE)?, nonce.to_string());
            let verifier = verifier(receiver, VerifyConfig::default())?;
            assert_eq!(
                verifier
                    .authenticate(&headers, &Method::POST, TARGET, BODY, nonce)?
                    .caller()
                    .nonce_ns,
                BigInt::from(nonce)
            );
            if nonce == 0 {
                assert_error(
                    verifier.authenticate(&headers, &Method::POST, TARGET, BODY, u64::MAX),
                    AuthError::StaleRequest("nonce is older than the 10s freshness window".into()),
                )?;
            } else if nonce == u64::MAX {
                assert_error(
                    verifier.authenticate(&headers, &Method::POST, TARGET, BODY, 0),
                    AuthError::StaleRequest("nonce is more than 2s in the future".into()),
                )?;
            }
        }
    }
    Ok(())
}

#[test]
fn nonce_text_is_parsed_then_signed_as_canonical_decimal() -> TestResult {
    let sender = alice(CryptoType::Ed25519)?;
    let receiver = bob()?;
    for text in [
        "+1752076800000000000",
        " 1752076800000000000 ",
        "01752076800000000000",
        "1_752_076_800_000_000_000",
    ] {
        let mut headers = headers(&sender, Some(&receiver))?;
        replace(&mut headers, HEADER_NONCE, text)?;
        assert_eq!(
            check(&headers, receiver)?.nonce_ns,
            BigInt::from(FIXED_NONCE)
        );
    }
    for text in ["", "1__2", "_1", "1_", "0x1", "+", "not-a-number"] {
        let mut headers = headers(&sender, Some(&receiver))?;
        replace(&mut headers, HEADER_NONCE, text)?;
        assert_error(
            verifier(receiver, VerifyConfig::default())?.authenticate(
                &headers,
                &Method::POST,
                TARGET,
                BODY,
                NOW,
            ),
            AuthError::MalformedAuth("X-Bittensor-Nonce is not a decimal integer".into()),
        )?;
    }
    let zero = block_on(sign(
        &sender,
        &Method::POST,
        TARGET,
        BODY,
        Some(&receiver),
        &FixedClock(0),
    ))?;
    for text in ["0", "+0", "-0", "-00", "0_0"] {
        let mut headers = zero.clone();
        replace(&mut headers, HEADER_NONCE, text)?;
        assert_eq!(
            verifier(receiver, VerifyConfig::default())?
                .authenticate(&headers, &Method::POST, TARGET, BODY, 0)?
                .caller()
                .nonce_ns,
            BigInt::from(0)
        );
    }
    Ok(())
}

#[test]
fn signed_and_oversized_decimal_nonces_reach_receiver_and_freshness_checks() -> TestResult {
    let sender = alice(CryptoType::Ed25519)?;
    let receiver = bob()?;
    let verifier = verifier(receiver, VerifyConfig::default())?;
    let huge = "9".repeat(300);
    for (nonce, stale) in [
        (
            "-1".to_string(),
            "nonce is older than the 10s freshness window",
        ),
        (
            "-18446744073709551616".to_string(),
            "nonce is older than the 10s freshness window",
        ),
        (
            format!("-{huge}"),
            "nonce is older than the 10s freshness window",
        ),
        (
            "18446744073709551616".to_string(),
            "nonce is more than 2s in the future",
        ),
        (huge, "nonce is more than 2s in the future"),
    ] {
        let mut request = headers(&sender, Some(&receiver))?;
        replace(&mut request, HEADER_NONCE, &nonce)?;
        assert_error(
            verifier.authenticate(&request, &Method::POST, TARGET, BODY, NOW),
            AuthError::StaleRequest(stale.into()),
        )?;
        replace(&mut request, HEADER_RECEIVER, &sender.account().to_string())?;
        assert_error(
            verifier.authenticate(&request, &Method::POST, TARGET, BODY, NOW),
            AuthError::WrongReceiver(WRONG_RECEIVER.into()),
        )?;
        request.remove(HEADER_RECEIVER);
        assert_error(
            verifier.authenticate(&request, &Method::POST, TARGET, BODY, NOW),
            AuthError::WrongReceiver(MISSING_RECEIVER.into()),
        )?;
        // Required shape checks still precede receiver and freshness checks.
        request.remove(HEADER_SIGNATURE);
        assert_error(
            verifier.authenticate(&request, &Method::POST, TARGET, BODY, NOW),
            AuthError::MalformedAuth("missing X-Bittensor-Signature".into()),
        )?;
    }
    Ok(())
}

#[test]
fn fresh_signed_and_oversized_nonce_values_reach_signature_verification() -> TestResult {
    let sender = alice(CryptoType::Ed25519)?;
    let receiver = bob()?;
    let verifier = verifier(receiver, VerifyConfig::default())?;
    for (nonce, now) in [("-1", 0), ("18446744073709551616", u64::MAX)] {
        let mut request = headers(&sender, Some(&receiver))?;
        replace(&mut request, HEADER_NONCE, nonce)?;
        assert_error(
            verifier.authenticate(&request, &Method::POST, TARGET, BODY, now),
            AuthError::BadSignature(BAD_SIGNATURE.into()),
        )?;
        replace(&mut request, HEADER_SIGNATURE, "0x00")?;
        assert_error(
            verifier.authenticate(&request, &Method::POST, TARGET, BODY, now),
            AuthError::MalformedAuth(INVALID_ENCODING.into()),
        )?;
    }
    Ok(())
}

#[test]
fn genuinely_signed_fresh_nonce_values_outside_u64_are_accepted_and_replay_protected() -> TestResult
{
    let receiver = bob()?;
    for crypto in ALGORITHMS {
        let sender = alice(crypto)?;
        for (nonce, normal_nonce, now) in
            [("-1", 0, 0), ("18446744073709551616", u64::MAX, u64::MAX)]
        {
            let payload = String::from_utf8(build_payload(
                crypto,
                &Method::POST,
                TARGET,
                BODY,
                normal_nonce,
                &sender.account(),
                Some(&receiver),
            ))?;
            let lines = payload
                .split('\n')
                .enumerate()
                .map(|(index, line)| if index == 5 { nonce } else { line })
                .collect::<Vec<_>>();
            let payload = lines.join("\n");
            let signature = block_on(sender.sign_raw(payload.as_bytes()))?;
            assert!(rtao_core::keys::verify_strict(
                payload.as_bytes(),
                &signature,
                &sender.account().to_string(),
                u8::from(crypto)
            )?);
            let mut request = headers(&sender, Some(&receiver))?;
            replace(&mut request, HEADER_NONCE, nonce)?;
            replace(
                &mut request,
                HEADER_SIGNATURE,
                &format!("0x{}", hex::encode(signature)),
            )?;
            let exact_nonce = nonce.parse::<BigInt>()?;
            assert!(BigInt::from(now) - &exact_nonce <= BigInt::from(DEFAULT_MAX_AGE.as_nanos()));
            assert!(
                &exact_nonce - BigInt::from(now) <= BigInt::from(DEFAULT_ALLOWED_SKEW.as_nanos())
            );
            assert!(u64::try_from(&exact_nonce).is_err());
            let store = Arc::new(ObservedStore::default());
            let verifier = Verifier::new(receiver, VerifyConfig::default(), Arc::clone(&store))?;
            let authenticated =
                verifier.authenticate(&request, &Method::POST, TARGET, BODY, now)?;
            assert_eq!(authenticated.caller().nonce_ns, exact_nonce);
            assert_eq!(store.calls.load(Ordering::SeqCst), 0);
            let caller = block_on(authenticated.commit(store.as_ref()))?;
            assert_eq!(caller.nonce_ns, exact_nonce);
            assert_eq!(caller.hotkey, sender.account());
            assert_eq!(caller.crypto, crypto);
            assert_eq!(store.calls.load(Ordering::SeqCst), 1);
            assert_error(
                block_on(verifier.verify(&request, &Method::POST, TARGET, BODY, now)),
                AuthError::ReplayedRequest(REPLAY.into()),
            )?;
            assert_eq!(store.calls.load(Ordering::SeqCst), 2);
            // Later failures retain the Python check precedence over replay.
            assert_error(
                block_on(verifier.verify(&request, &Method::POST, TARGET, b"tampered", now)),
                AuthError::BadSignature(BAD_SIGNATURE.into()),
            )?;
            replace(&mut request, HEADER_RECEIVER, &sender.account().to_string())?;
            assert_error(
                block_on(verifier.verify(&request, &Method::POST, TARGET, BODY, now)),
                AuthError::WrongReceiver(WRONG_RECEIVER.into()),
            )?;
            assert_eq!(store.calls.load(Ordering::SeqCst), 2);
        }
    }
    Ok(())
}

#[test]
fn unicode_nonce_spellings_preserve_signature_and_share_replay_identity() -> TestResult {
    let receiver = bob()?;
    for crypto in ALGORITHMS {
        let sender = alice(crypto)?;
        let canonical = headers(&sender, Some(&receiver))?;
        let digits = FIXED_NONCE.to_string();
        for zero in ['٠', '０', '𝟎', '𝟘', '𝟢', '𝟬', '𝟶'] {
            let unicode = digits
                .chars()
                .map(|digit| {
                    char::from_u32(u32::from(zero) + u32::from(digit) - u32::from('0'))
                        .ok_or_else(|| io::Error::other("invalid decimal test scalar"))
                })
                .collect::<Result<String, _>>()?;
            let mut request = canonical.clone();
            replace(
                &mut request,
                HEADER_NONCE,
                &format!("\u{a0}+{unicode}\u{202f}"),
            )?;
            let verifier = verifier(receiver, VerifyConfig::default())?;
            let caller = block_on(verifier.verify(&request, &Method::POST, TARGET, BODY, NOW))?;
            assert_eq!(caller.nonce_ns, BigInt::from(FIXED_NONCE));
            assert_error(
                block_on(verifier.verify(&canonical, &Method::POST, TARGET, BODY, NOW)),
                AuthError::ReplayedRequest(REPLAY.into()),
            )?;
        }
        let mut mixed = canonical.clone();
        replace(
            &mut mixed,
            HEADER_NONCE,
            "١_７_𝟓_2_٠_𝟟_６_٨_０_0_٠_０_𝟢_0_٠_０_𝟎_0_٠",
        )?;
        assert_eq!(check(&mixed, receiver)?.nonce_ns, BigInt::from(FIXED_NONCE));
        for bad in [
            "²",
            "①",
            "Ⅳ",
            "½",
            "١__７",
            "١_",
            "_١",
            "١ ７",
            "１＿２",
            "＋١",
        ] {
            let mut request = canonical.clone();
            replace(&mut request, HEADER_NONCE, bad)?;
            assert_error(
                verifier(receiver, VerifyConfig::default())?.authenticate(
                    &request,
                    &Method::POST,
                    TARGET,
                    BODY,
                    NOW,
                ),
                AuthError::MalformedAuth("X-Bittensor-Nonce is not a decimal integer".into()),
            )?;
        }
    }
    Ok(())
}

#[test]
fn unicode_decimal_crypto_aliases_choose_only_the_declared_algorithm() -> TestResult {
    let receiver = bob()?;
    for (crypto, aliases) in [
        (
            CryptoType::Ed25519,
            ["٠", "０", "𝟎", "𝟘", "𝟢", "\u{a0}٠０𝟎\u{202f}"],
        ),
        (
            CryptoType::Sr25519,
            ["١", "１", "𝟏", "𝟙", "𝟣", "\u{a0}٠０𝟏\u{202f}"],
        ),
    ] {
        let sender = alice(crypto)?;
        for alias in aliases {
            let mut request = headers(&sender, Some(&receiver))?;
            replace(&mut request, HEADER_CRYPTO, alias)?;
            assert_eq!(check(&request, receiver)?.crypto, crypto);
        }
        for invalid in ["²", "①", "½", "٠_١", "١_", "+١", "-٠", "٢", "𝟡"] {
            let mut request = headers(&sender, Some(&receiver))?;
            replace(&mut request, HEADER_CRYPTO, invalid)?;
            assert_error(
                verifier(receiver, VerifyConfig::default())?.authenticate(
                    &request,
                    &Method::POST,
                    TARGET,
                    BODY,
                    NOW,
                ),
                AuthError::MalformedAuth("unsupported X-Bittensor-Crypto".into()),
            )?;
        }
        let mut confused = headers(&sender, Some(&receiver))?;
        replace(
            &mut confused,
            HEADER_CRYPTO,
            if crypto == CryptoType::Ed25519 {
                "١"
            } else {
                "٠"
            },
        )?;
        assert_error(
            verifier(receiver, VerifyConfig::default())?.authenticate(
                &confused,
                &Method::POST,
                TARGET,
                BODY,
                NOW,
            ),
            AuthError::BadSignature(BAD_SIGNATURE.into()),
        )?;
    }
    Ok(())
}

// pyt: unit/test_http_auth.py::test_replay_rejected
#[test]
fn test_replay_rejected() -> TestResult {
    let receiver = bob()?;
    for crypto in ALGORITHMS {
        let sender = alice(crypto)?;
        let headers = headers(&sender, Some(&receiver))?;
        let verifier = verifier(receiver, VerifyConfig::default())?;
        assert_eq!(
            block_on(verifier.verify(&headers, &Method::POST, TARGET, BODY, NOW))?.hotkey,
            sender.account()
        );
        assert_error(
            block_on(verifier.verify(&headers, &Method::POST, TARGET, BODY, NOW)),
            AuthError::ReplayedRequest(REPLAY.into()),
        )?;
    }
    Ok(())
}

#[test]
fn concurrent_authenticated_requests_have_exactly_one_replay_commit_winner() -> TestResult {
    let sender = alice(CryptoType::Sr25519)?;
    let receiver = bob()?;
    let headers = headers(&sender, Some(&receiver))?;
    let verifier = Arc::new(verifier(receiver, VerifyConfig::default())?);
    let barrier = Arc::new(Barrier::new(17));
    let handles: Vec<_> = (0..16)
        .map(|_| {
            let headers = headers.clone();
            let verifier = verifier.clone();
            let barrier = barrier.clone();
            thread::spawn(move || {
                barrier.wait();
                block_on(verifier.verify(&headers, &Method::POST, TARGET, BODY, NOW))
            })
        })
        .collect();
    barrier.wait();
    let mut accepted = 0;
    let mut replays = 0;
    for handle in handles {
        match handle
            .join()
            .map_err(|_| io::Error::other("verification worker panicked"))?
        {
            Ok(caller) => {
                accepted += 1;
                assert_eq!(
                    caller,
                    Caller {
                        hotkey: sender.account(),
                        nonce_ns: BigInt::from(FIXED_NONCE),
                        crypto: CryptoType::Sr25519
                    }
                );
            }
            Err(error) => {
                assert_eq!(error, AuthError::ReplayedRequest(REPLAY.into()));
                replays += 1;
            }
        }
    }
    assert_eq!((accepted, replays), (1, 15));
    Ok(())
}

// pyt: unit/test_http_auth.py::test_window_wider_than_store_retention_refused
#[test]
fn test_window_wider_than_store_retention_refused() -> TestResult {
    let receiver = bob()?;
    let cfg = VerifyConfig {
        max_age: Duration::from_secs(120),
        ..Default::default()
    };
    let error = verifier(receiver, cfg.clone())
        .err()
        .ok_or_else(|| io::Error::other("short retention accepted"))?;
    assert_eq!(
        error.to_string(),
        "nonce store retains entries for 60s but the inclusive freshness window is 122s; use retention > max_age + allowed_skew"
    );
    let store = Arc::new(InMemoryNonceStore::with_config(NonceStoreConfig {
        retention: Duration::from_secs(300),
        ..Default::default()
    })?);
    let verifier = Verifier::new(receiver, cfg, store)?;
    let sender = alice(CryptoType::Sr25519)?;
    let headers = headers(&sender, Some(&receiver))?;
    assert_eq!(
        block_on(verifier.verify(&headers, &Method::POST, TARGET, BODY, NOW))?.hotkey,
        sender.account()
    );
    // At equality a store can expire a future-bound nonce while freshness
    // still accepts it. The constructor must refuse this configuration.
    let exact_store = Arc::new(InMemoryNonceStore::with_config(NonceStoreConfig {
        retention: Duration::from_secs(12),
        ..Default::default()
    })?);
    let error = Verifier::new(receiver, VerifyConfig::default(), exact_store)
        .err()
        .ok_or_else(|| io::Error::other("inclusive retention boundary accepted"))?;
    assert_eq!(
        error.to_string(),
        "nonce store retains entries for 12s but the inclusive freshness window is 12s; use retention > max_age + allowed_skew"
    );
    let protected_store = Arc::new(InMemoryNonceStore::with_config(NonceStoreConfig {
        retention: Duration::from_secs(12) + Duration::from_nanos(1),
        ..Default::default()
    })?);
    assert!(Verifier::new(receiver, VerifyConfig::default(), protected_store).is_ok());
    let short_store = Arc::new(InMemoryNonceStore::with_config(NonceStoreConfig {
        retention: Duration::from_secs(12) - Duration::from_nanos(1),
        ..Default::default()
    })?);
    assert!(Verifier::new(receiver, VerifyConfig::default(), short_store).is_err());
    Ok(())
}

#[test]
fn fractional_freshness_boundaries_and_messages_match_python_general_format() -> TestResult {
    let sender = alice(CryptoType::Ed25519)?;
    let receiver = bob()?;
    let headers = headers(&sender, Some(&receiver))?;
    for (age_ns, skew_ns, age_text, skew_text) in [
        (1_250_000_000, 500_000_000, "1.25", "0.5"),
        (10_000, 100_000, "1e-05", "0.0001"),
    ] {
        let verifier = verifier(
            receiver,
            VerifyConfig {
                max_age: Duration::from_nanos(age_ns),
                allowed_skew: Duration::from_nanos(skew_ns),
                ..Default::default()
            },
        )?;
        for now in [FIXED_NONCE + age_ns, FIXED_NONCE - skew_ns] {
            assert_eq!(
                verifier
                    .authenticate(&headers, &Method::POST, TARGET, BODY, now)?
                    .caller()
                    .nonce_ns,
                BigInt::from(FIXED_NONCE)
            );
        }
        assert_error(
            verifier.authenticate(
                &headers,
                &Method::POST,
                TARGET,
                BODY,
                FIXED_NONCE + age_ns + 1,
            ),
            AuthError::StaleRequest(format!(
                "nonce is older than the {age_text}s freshness window"
            )),
        )?;
        assert_error(
            verifier.authenticate(
                &headers,
                &Method::POST,
                TARGET,
                BODY,
                FIXED_NONCE - skew_ns - 1,
            ),
            AuthError::StaleRequest(format!("nonce is more than {skew_text}s in the future")),
        )?;
    }
    Ok(())
}

// pyt: unit/test_http_auth.py::test_nonce_store_expires_entries
#[test]
fn test_nonce_store_expires_entries() -> TestResult {
    let store = InMemoryNonceStore::with_config(NonceStoreConfig {
        retention: Duration::ZERO,
        ..Default::default()
    })?;
    let hotkey = bob()?;
    assert!(block_on(store.check_and_store(&hotkey, &BigInt::from(1)))?);
    assert!(block_on(store.check_and_store(&hotkey, &BigInt::from(1)))?);
    Ok(())
}

// pyt: unit/test_http_auth.py::test_tampered_nonce_rejected
#[test]
fn test_tampered_nonce_rejected() -> TestResult {
    let receiver = bob()?;
    for crypto in ALGORITHMS {
        let sender = alice(crypto)?;
        let mut headers = headers(&sender, Some(&receiver))?;
        replace(&mut headers, HEADER_NONCE, &(FIXED_NONCE + 1).to_string())?;
        assert_error(
            verifier(receiver, VerifyConfig::default())?.authenticate(
                &headers,
                &Method::POST,
                TARGET,
                BODY,
                FIXED_NONCE + 2,
            ),
            AuthError::BadSignature(BAD_SIGNATURE.into()),
        )?;
    }
    Ok(())
}

// pyt: unit/test_http_auth.py::test_tampered_body_method_path_rejected
#[test]
fn test_tampered_body_method_path_rejected() -> TestResult {
    let receiver = bob()?;
    for crypto in ALGORITHMS {
        let sender = alice(crypto)?;
        let original = headers(&sender, Some(&receiver))?;
        let verifier = verifier(receiver, VerifyConfig::default())?;
        for result in [
            verifier.authenticate(&original, &Method::POST, TARGET, b"tampered body", NOW),
            verifier.authenticate(&original, &Method::DELETE, TARGET, BODY, NOW),
            verifier.authenticate(&original, &Method::POST, "/admin", BODY, NOW),
        ] {
            assert_error(result, AuthError::BadSignature(BAD_SIGNATURE.into()))?;
        }
    }
    Ok(())
}

// pyt: unit/test_http_auth.py::test_scheme_confusion_rejected
#[test]
fn test_scheme_confusion_rejected() -> TestResult {
    let receiver = bob()?;
    for crypto in ALGORITHMS {
        let sender = alice(crypto)?;
        let mut confused = headers(&sender, Some(&receiver))?;
        if crypto == CryptoType::Ed25519 {
            confused.remove(HEADER_CRYPTO);
        } else {
            replace(&mut confused, HEADER_CRYPTO, "ed25519")?;
        }
        assert_error(
            verifier(receiver, VerifyConfig::default())?.authenticate(
                &confused,
                &Method::POST,
                TARGET,
                BODY,
                NOW,
            ),
            AuthError::BadSignature(BAD_SIGNATURE.into()),
        )?;
    }
    Ok(())
}

// pyt: unit/test_http_auth.py::test_wrong_receiver_rejected
#[test]
fn test_wrong_receiver_rejected() -> TestResult {
    let receiver = bob()?;
    for crypto in ALGORITHMS {
        let sender = alice(crypto)?;
        let bound = headers(&sender, Some(&receiver))?;
        assert_error(
            verifier(sender.account(), VerifyConfig::default())?.authenticate(
                &bound,
                &Method::POST,
                TARGET,
                BODY,
                NOW,
            ),
            AuthError::WrongReceiver(WRONG_RECEIVER.into()),
        )?;
    }
    Ok(())
}

// pyt: unit/test_http_auth.py::test_unbound_request_requires_opt_out
#[test]
fn test_unbound_request_requires_opt_out() -> TestResult {
    let receiver = bob()?;
    for crypto in ALGORITHMS {
        let sender = alice(crypto)?;
        let bound = headers(&sender, Some(&receiver))?;
        let unbound = headers(&sender, None)?;
        assert!(!unbound.contains_key(HEADER_RECEIVER));
        assert_error(
            verifier(receiver, VerifyConfig::default())?.authenticate(
                &unbound,
                &Method::POST,
                TARGET,
                BODY,
                NOW,
            ),
            AuthError::WrongReceiver(MISSING_RECEIVER.into()),
        )?;
        let cfg = VerifyConfig {
            require_receiver: false,
            ..Default::default()
        };
        let permissive = verifier(receiver, cfg)?;
        assert_eq!(
            block_on(permissive.verify(&unbound, &Method::POST, TARGET, BODY, NOW))?.hotkey,
            sender.account()
        );
        // Opting out of required binding does not permit an incorrect binding.
        let mut wrong = bound.clone();
        replace(&mut wrong, HEADER_RECEIVER, &sender.account().to_string())?;
        assert_error(
            permissive.authenticate(&wrong, &Method::POST, TARGET, BODY, NOW),
            AuthError::WrongReceiver(WRONG_RECEIVER.into()),
        )?;
        let unbound_payload = build_payload(
            crypto,
            &Method::POST,
            TARGET,
            BODY,
            FIXED_NONCE,
            &sender.account(),
            None,
        );
        assert_eq!(
            std::str::from_utf8(&unbound_payload)?.lines().last(),
            Some("-")
        );
    }
    Ok(())
}

// pyt: unit/test_http_auth.py::test_missing_header_rejected
#[test]
fn test_missing_header_rejected() -> TestResult {
    let sender = alice(CryptoType::Sr25519)?;
    let receiver = bob()?;
    for (missing, message) in [
        (
            HEADER_VERSION,
            "missing or unsupported X-Bittensor-Version (expected '1')",
        ),
        (HEADER_HOTKEY, "missing X-Bittensor-Hotkey"),
        (HEADER_NONCE, "missing X-Bittensor-Nonce"),
        (HEADER_SIGNATURE, "missing X-Bittensor-Signature"),
    ] {
        let mut headers = headers(&sender, Some(&receiver))?;
        headers.remove(missing);
        assert_error(
            verifier(receiver, VerifyConfig::default())?.authenticate(
                &headers,
                &Method::POST,
                TARGET,
                BODY,
                NOW,
            ),
            AuthError::MalformedAuth(message.into()),
        )?;
    }
    Ok(())
}

// pyt: unit/test_http_auth.py::test_unparseable_header_rejected
#[test]
fn test_unparseable_header_rejected() -> TestResult {
    let sender = alice(CryptoType::Sr25519)?;
    let receiver = bob()?;
    for (header, value, message) in [
        (
            HEADER_NONCE,
            "not-a-number",
            "X-Bittensor-Nonce is not a decimal integer",
        ),
        (HEADER_CRYPTO, "ecdsa", "unsupported X-Bittensor-Crypto"),
        (HEADER_CRYPTO, "2", "unsupported X-Bittensor-Crypto"),
        (HEADER_CRYPTO, "256", "unsupported X-Bittensor-Crypto"),
        (HEADER_CRYPTO, "", "unsupported X-Bittensor-Crypto"),
        (
            HEADER_SIGNATURE,
            "0xnothex",
            "X-Bittensor-Signature is not hex",
        ),
        (HEADER_SIGNATURE, "0x1", "X-Bittensor-Signature is not hex"),
        (HEADER_SIGNATURE, "0X00", "X-Bittensor-Signature is not hex"),
        (
            HEADER_VERSION,
            "2",
            "missing or unsupported X-Bittensor-Version (expected '1')",
        ),
        (HEADER_HOTKEY, "", "missing X-Bittensor-Hotkey"),
    ] {
        let mut headers = headers(&sender, Some(&receiver))?;
        replace(&mut headers, header, value)?;
        assert_error(
            verifier(receiver, VerifyConfig::default())?.authenticate(
                &headers,
                &Method::POST,
                TARGET,
                BODY,
                NOW,
            ),
            AuthError::MalformedAuth(message.into()),
        )?;
    }
    Ok(())
}

#[test]
fn valid_hex_with_invalid_signature_length_and_bad_ss58_are_malformed_for_both_policies()
-> TestResult {
    let receiver = bob()?;
    for crypto in ALGORITHMS {
        let sender = alice(crypto)?;
        let original = headers(&sender, Some(&receiver))?;
        for policy in [WrappedPolicy::AcceptLikePython, WrappedPolicy::Strict] {
            let verifier = verifier(
                receiver,
                VerifyConfig {
                    wrapped: policy,
                    ..Default::default()
                },
            )?;
            for length in [0, 1, 63, 65, 128] {
                let mut headers = original.clone();
                replace(
                    &mut headers,
                    HEADER_SIGNATURE,
                    &format!("0x{}", hex::encode(vec![0u8; length])),
                )?;
                assert_error(
                    verifier.authenticate(&headers, &Method::POST, TARGET, BODY, NOW),
                    AuthError::MalformedAuth(INVALID_ENCODING.into()),
                )?;
            }
            for address in [
                "not-an-ss58-address",
                "5F...",
                "0x012345",
                "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQX",
            ] {
                let mut headers = original.clone();
                replace(&mut headers, HEADER_HOTKEY, address)?;
                assert_error(
                    verifier.authenticate(&headers, &Method::POST, TARGET, BODY, NOW),
                    AuthError::MalformedAuth(INVALID_ENCODING.into()),
                )?;
            }
            let mut headers = original.clone();
            replace(
                &mut headers,
                HEADER_SIGNATURE,
                &format!("0x{}", hex::encode([0u8; 64])),
            )?;
            assert_error(
                verifier.authenticate(&headers, &Method::POST, TARGET, BODY, NOW),
                AuthError::BadSignature(BAD_SIGNATURE.into()),
            )?;
        }
    }
    Ok(())
}

#[test]
fn signature_hex_accepts_python_whitespace_uppercase_and_unprefixed_forms() -> TestResult {
    let receiver = bob()?;
    for crypto in ALGORITHMS {
        let sender = alice(crypto)?;
        let original = headers(&sender, Some(&receiver))?;
        let signature = header_text(&original, HEADER_SIGNATURE)?
            .strip_prefix("0x")
            .ok_or_else(|| io::Error::other("missing generated hex prefix"))?;
        let pairs: Vec<_> = signature
            .as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| std::str::from_utf8(pair))
            .collect::<Result<_, _>>()?;
        for spelling in [
            signature.to_string(),
            signature.to_uppercase(),
            format!("0x {} \t", pairs.join(" ")),
        ] {
            let mut headers = original.clone();
            replace(&mut headers, HEADER_SIGNATURE, &spelling)?;
            assert_eq!(check(&headers, receiver)?.hotkey, sender.account());
        }
        let mut headers = original.clone();
        let split_nibble = format!(
            "0x{} {}",
            signature
                .get(..1)
                .ok_or_else(|| io::Error::other("empty signature"))?,
            signature
                .get(1..)
                .ok_or_else(|| io::Error::other("empty signature"))?
        );
        replace(&mut headers, HEADER_SIGNATURE, &split_nibble)?;
        assert_error(
            verifier(receiver, VerifyConfig::default())?.authenticate(
                &headers,
                &Method::POST,
                TARGET,
                BODY,
                NOW,
            ),
            AuthError::MalformedAuth("X-Bittensor-Signature is not hex".into()),
        )?;
    }
    Ok(())
}

#[test]
fn wrapped_signatures_are_accepted_by_default_and_rejected_in_strict_mode_for_both_algorithms()
-> TestResult {
    assert_eq!(WrappedPolicy::default(), WrappedPolicy::AcceptLikePython);
    assert_eq!(
        VerifyConfig::default().wrapped,
        WrappedPolicy::AcceptLikePython
    );
    assert_eq!(VerifyConfig::default().max_age, DEFAULT_MAX_AGE);
    assert_eq!(VerifyConfig::default().allowed_skew, DEFAULT_ALLOWED_SKEW);
    assert!(VerifyConfig::default().require_receiver);
    let receiver = bob()?;
    for crypto in ALGORITHMS {
        let sender = alice(crypto)?;
        let mut headers = headers(&sender, Some(&receiver))?;
        let strict = verifier(
            receiver,
            VerifyConfig {
                wrapped: WrappedPolicy::Strict,
                ..Default::default()
            },
        )?;
        assert_eq!(
            strict
                .authenticate(&headers, &Method::POST, TARGET, BODY, NOW)?
                .caller()
                .crypto,
            crypto
        );
        let payload = build_payload(
            crypto,
            &Method::POST,
            TARGET,
            BODY,
            FIXED_NONCE,
            &sender.account(),
            Some(&receiver),
        );
        let wrapped = [b"<Bytes>".as_slice(), &payload, b"</Bytes>"].concat();
        let signature = block_on(sender.sign_raw(&wrapped))?;
        assert!(!rtao_core::keys::verify_strict(
            &payload,
            &signature,
            &sender.account().to_string(),
            u8::from(crypto)
        )?);
        assert!(rtao_core::keys::verify_strict(
            &wrapped,
            &signature,
            &sender.account().to_string(),
            u8::from(crypto)
        )?);
        replace(
            &mut headers,
            HEADER_SIGNATURE,
            &format!("0x{}", hex::encode(signature)),
        )?;
        assert_eq!(check(&headers, receiver)?.hotkey, sender.account());
        assert_error(
            strict.authenticate(&headers, &Method::POST, TARGET, BODY, NOW),
            AuthError::BadSignature(BAD_SIGNATURE.into()),
        )?;
        assert_error(
            verifier(receiver, VerifyConfig::default())?.authenticate(
                &headers,
                &Method::POST,
                TARGET,
                b"changed",
                NOW,
            ),
            AuthError::BadSignature(BAD_SIGNATURE.into()),
        )?;
    }
    Ok(())
}

#[test]
fn alternate_sender_ss58_prefix_preserves_exact_signed_text_and_normalizes_replay_identity()
-> TestResult {
    let receiver = bob()?;
    for crypto in ALGORITHMS {
        let sender = alice(crypto)?;
        let canonical = sender.account().to_string();
        let alternate = sender.account().to_ss58(0);
        assert_ne!(canonical, alternate);
        assert_eq!(alternate.parse::<AccountId>()?, sender.account());
        let original = headers(&sender, Some(&receiver))?;
        let payload = String::from_utf8(build_payload(
            crypto,
            &Method::POST,
            TARGET,
            BODY,
            FIXED_NONCE,
            &sender.account(),
            Some(&receiver),
        ))?;
        let alternate_payload =
            payload.replace(&format!("\n{canonical}\n"), &format!("\n{alternate}\n"));
        assert_ne!(payload, alternate_payload);
        let alternate_signature = block_on(sender.sign_raw(alternate_payload.as_bytes()))?;
        let mut alternate_headers = original.clone();
        replace(&mut alternate_headers, HEADER_HOTKEY, &alternate)?;
        let verifier = verifier(receiver, VerifyConfig::default())?;
        // Changing only the address spelling invalidates the signed raw text.
        assert_error(
            verifier.authenticate(&alternate_headers, &Method::POST, TARGET, BODY, NOW),
            AuthError::BadSignature(BAD_SIGNATURE.into()),
        )?;
        replace(
            &mut alternate_headers,
            HEADER_SIGNATURE,
            &format!("0x{}", hex::encode(alternate_signature)),
        )?;
        assert_eq!(
            block_on(verifier.verify(&alternate_headers, &Method::POST, TARGET, BODY, NOW))?.hotkey,
            sender.account()
        );
        // The same public key and nonce cannot replay under another prefix.
        assert_error(
            block_on(verifier.verify(&original, &Method::POST, TARGET, BODY, NOW)),
            AuthError::ReplayedRequest(REPLAY.into()),
        )?;
    }
    Ok(())
}

#[derive(Default)]
struct ObservedStore {
    calls: AtomicUsize,
    inner: InMemoryNonceStore,
}

impl NonceStore for ObservedStore {
    fn retention(&self) -> Duration {
        self.inner.retention()
    }

    fn check_and_store<'a>(
        &'a self,
        hotkey: &'a AccountId,
        nonce_ns: &'a BigInt,
    ) -> BoxFuture<'a, Result<bool, StoreError>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.inner.check_and_store(hotkey, nonce_ns).await
        })
    }
}

#[test]
fn no_replay_store_access_occurs_before_authenticity() -> TestResult {
    let sender = alice(CryptoType::Sr25519)?;
    let receiver = bob()?;
    let original = headers(&sender, Some(&receiver))?;
    let store = Arc::new(ObservedStore::default());
    let verifier = Verifier::new(receiver, VerifyConfig::default(), store.clone())?;
    let mut malformed = original.clone();
    malformed.remove(HEADER_SIGNATURE);
    assert_error(
        block_on(verifier.verify(&malformed, &Method::POST, TARGET, BODY, NOW)),
        AuthError::MalformedAuth("missing X-Bittensor-Signature".into()),
    )?;
    let mut wrong = original.clone();
    replace(&mut wrong, HEADER_RECEIVER, &sender.account().to_string())?;
    assert_error(
        block_on(verifier.verify(&wrong, &Method::POST, TARGET, BODY, NOW)),
        AuthError::WrongReceiver(WRONG_RECEIVER.into()),
    )?;
    assert_error(
        block_on(verifier.verify(
            &original,
            &Method::POST,
            TARGET,
            BODY,
            FIXED_NONCE + 11_000_000_000,
        )),
        AuthError::StaleRequest("nonce is older than the 10s freshness window".into()),
    )?;
    assert_error(
        block_on(verifier.verify(&original, &Method::POST, TARGET, b"tampered", NOW)),
        AuthError::BadSignature(BAD_SIGNATURE.into()),
    )?;
    let mut invalid_address = original.clone();
    replace(&mut invalid_address, HEADER_HOTKEY, "not-a-hotkey")?;
    assert_error(
        block_on(verifier.verify(&invalid_address, &Method::POST, TARGET, BODY, NOW)),
        AuthError::MalformedAuth(INVALID_ENCODING.into()),
    )?;
    assert_eq!(store.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        block_on(verifier.verify(&original, &Method::POST, TARGET, BODY, NOW))?.hotkey,
        sender.account()
    );
    assert_eq!(store.calls.load(Ordering::SeqCst), 1);
    assert_error(
        block_on(verifier.verify(&original, &Method::POST, TARGET, BODY, NOW)),
        AuthError::ReplayedRequest(REPLAY.into()),
    )?;
    assert_eq!(store.calls.load(Ordering::SeqCst), 2);
    Ok(())
}

#[test]
fn authenticate_then_policy_then_commit_preserves_denied_requests_and_checks_replay_last()
-> TestResult {
    let sender = alice(CryptoType::Ed25519)?;
    let receiver = bob()?;
    let headers = headers(&sender, Some(&receiver))?;
    let store = Arc::new(ObservedStore::default());
    let verifier = Verifier::new(receiver, VerifyConfig::default(), store.clone())?;
    {
        let denied = verifier.authenticate(&headers, &Method::POST, TARGET, BODY, NOW)?;
        assert_eq!(
            *denied.caller(),
            Caller {
                hotkey: sender.account(),
                nonce_ns: BigInt::from(FIXED_NONCE),
                crypto: CryptoType::Ed25519
            }
        );
        assert_eq!(store.calls.load(Ordering::SeqCst), 0);
        // A policy rejection discards the authenticated value without committing.
    }
    assert_eq!(store.calls.load(Ordering::SeqCst), 0);
    let allowed = verifier.authenticate(&headers, &Method::POST, TARGET, BODY, NOW)?;
    let allowed_hotkeys = [sender.account()];
    assert!(allowed_hotkeys.contains(&allowed.caller().hotkey));
    assert_eq!(store.calls.load(Ordering::SeqCst), 0);
    let caller = block_on(allowed.commit(store.as_ref()))?;
    assert_eq!(caller.hotkey, sender.account());
    assert_eq!(store.calls.load(Ordering::SeqCst), 1);
    // authenticate remains stateless even when this nonce is already a replay.
    let replay = verifier.authenticate(&headers, &Method::POST, TARGET, BODY, NOW)?;
    assert_eq!(store.calls.load(Ordering::SeqCst), 1);
    assert_error(
        block_on(replay.commit(store.as_ref())),
        AuthError::ReplayedRequest(REPLAY.into()),
    )?;
    assert_eq!(store.calls.load(Ordering::SeqCst), 2);
    Ok(())
}

#[test]
fn python_validation_precedence_is_shape_receiver_freshness_signature_then_replay() -> TestResult {
    let sender = alice(CryptoType::Sr25519)?;
    let receiver = bob()?;
    let original = headers(&sender, Some(&receiver))?;
    let verifier = verifier(receiver, VerifyConfig::default())?;
    block_on(verifier.verify(&original, &Method::POST, TARGET, BODY, NOW))?;
    let mut headers = original.clone();
    replace(&mut headers, HEADER_RECEIVER, &sender.account().to_string())?;
    replace(&mut headers, HEADER_HOTKEY, "invalid-address")?;
    replace(&mut headers, HEADER_VERSION, "2")?;
    let stale_now = FIXED_NONCE + 11_000_000_000;
    assert_error(
        block_on(verifier.verify(&headers, &Method::POST, TARGET, b"tampered", stale_now)),
        AuthError::MalformedAuth(
            "missing or unsupported X-Bittensor-Version (expected '1')".into(),
        ),
    )?;
    replace(&mut headers, HEADER_VERSION, "1")?;
    assert_error(
        block_on(verifier.verify(&headers, &Method::POST, TARGET, b"tampered", stale_now)),
        AuthError::WrongReceiver(WRONG_RECEIVER.into()),
    )?;
    replace(&mut headers, HEADER_RECEIVER, &receiver.to_string())?;
    assert_error(
        block_on(verifier.verify(&headers, &Method::POST, TARGET, b"tampered", stale_now)),
        AuthError::StaleRequest("nonce is older than the 10s freshness window".into()),
    )?;
    assert_error(
        block_on(verifier.verify(&headers, &Method::POST, TARGET, b"tampered", NOW)),
        AuthError::MalformedAuth(INVALID_ENCODING.into()),
    )?;
    replace(&mut headers, HEADER_HOTKEY, &sender.account().to_string())?;
    assert_error(
        block_on(verifier.verify(&headers, &Method::POST, TARGET, b"tampered", NOW)),
        AuthError::BadSignature(BAD_SIGNATURE.into()),
    )?;
    assert_error(
        block_on(verifier.verify(&headers, &Method::POST, TARGET, BODY, NOW)),
        AuthError::ReplayedRequest(REPLAY.into()),
    )?;
    Ok(())
}

#[test]
fn non_text_header_values_are_malformed_before_crypto_or_store_access() -> TestResult {
    let sender = alice(CryptoType::Sr25519)?;
    let receiver = bob()?;
    for header in [
        HEADER_VERSION,
        HEADER_HOTKEY,
        HEADER_NONCE,
        HEADER_CRYPTO,
        HEADER_SIGNATURE,
        HEADER_RECEIVER,
    ] {
        let mut headers = headers(&sender, Some(&receiver))?;
        headers.insert(
            HeaderName::from_bytes(header.as_bytes())?,
            HeaderValue::from_bytes(b"\xff")?,
        );
        assert_error(
            verifier(receiver, VerifyConfig::default())?.authenticate(
                &headers,
                &Method::POST,
                TARGET,
                BODY,
                NOW,
            ),
            AuthError::MalformedAuth(format!("{header} is not valid text")),
        )?;
    }
    Ok(())
}

// pyt: unit/test_http_auth.py::test_auth_errors_root_in_bittensor_error
#[test]
fn test_auth_errors_root_in_bittensor_error() {
    // Rust's shared error contract replaces Python's BittensorError hierarchy.
    for error in [
        AuthError::MalformedAuth("malformed".into()),
        AuthError::WrongReceiver("receiver".into()),
        AuthError::StaleRequest("stale".into()),
        AuthError::ReplayedRequest("replay".into()),
        AuthError::BadSignature("signature".into()),
    ] {
        let root: &dyn Error = &error;
        assert_eq!(root.to_string(), error.to_string());
        assert!(root.source().is_none());
    }
}

struct CountingSigner {
    inner: KeypairSigner,
    raw_calls: AtomicUsize,
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
        self.inner.sign(request)
    }

    fn sign_raw<'a>(&'a self, message: &'a [u8]) -> BoxFuture<'a, Result<[u8; 64], SignerError>> {
        self.raw_calls.fetch_add(1, Ordering::SeqCst);
        self.inner.sign_raw(message)
    }
}

struct FailingClock(ClockError);

impl Clock for FailingClock {
    fn now_ns(&self) -> Result<u64, ClockError> {
        Err(self.0)
    }
}

#[test]
fn checked_clock_errors_fail_before_signing_without_saturating_a_nonce() -> TestResult {
    let receiver = bob()?;
    let sender = CountingSigner {
        inner: alice(CryptoType::Ed25519)?,
        raw_calls: AtomicUsize::new(0),
    };
    for (clock_error, message) in [
        (
            ClockError::BeforeUnixEpoch,
            "system clock precedes the Unix epoch",
        ),
        (
            ClockError::NonceOverflow,
            "Unix nanosecond nonce exceeds u64",
        ),
    ] {
        let result = block_on(sign(
            &sender,
            &Method::POST,
            TARGET,
            BODY,
            Some(&receiver),
            &FailingClock(clock_error),
        ));
        assert_error(result, AuthError::Clock(clock_error))?;
        assert_eq!(clock_error.to_string(), message);
        assert_eq!(
            AuthError::Clock(clock_error)
                .source()
                .map(ToString::to_string),
            Some(message.to_string())
        );
        assert_eq!(sender.raw_calls.load(Ordering::SeqCst), 0);
    }
    let headers = headers(&sender, Some(&receiver))?;
    assert_eq!(sender.raw_calls.load(Ordering::SeqCst), 1);
    assert_eq!(check(&headers, receiver)?.hotkey, sender.account());
    Ok(())
}

#[test]
fn shared_live_clock_produces_unique_nonces_on_normal_threads() -> TestResult {
    let clock = Arc::new(SystemClock::default());
    let barrier = Arc::new(Barrier::new(17));
    let handles: Vec<_> = (0..16)
        .map(|_| {
            let clock = clock.clone();
            let barrier = barrier.clone();
            thread::spawn(move || {
                barrier.wait();
                (0..128)
                    .map(|_| clock.now_ns())
                    .collect::<Result<Vec<_>, _>>()
            })
        })
        .collect();
    barrier.wait();
    let mut nonces = Vec::new();
    for handle in handles {
        let thread_nonces = handle
            .join()
            .map_err(|_| io::Error::other("clock worker panicked"))??;
        assert!(
            thread_nonces
                .windows(2)
                .all(|pair| matches!(pair, [first, second] if first < second))
        );
        nonces.extend(thread_nonces);
    }
    assert_eq!(nonces.len(), 2_048);
    nonces.sort_unstable();
    nonces.dedup();
    assert_eq!(nonces.len(), 2_048);
    assert!(
        nonces
            .last()
            .is_some_and(|last| clock.now_ns().is_ok_and(|next| next > *last))
    );
    Ok(())
}

struct TransactionOnlySigner(AccountId);

impl Signer for TransactionOnlySigner {
    fn account(&self) -> AccountId {
        self.0
    }

    fn crypto_type(&self) -> CryptoType {
        CryptoType::Sr25519
    }

    fn sign<'a>(
        &'a self,
        _request: &'a SigningRequest<'a>,
    ) -> BoxFuture<'a, Result<MultiSignature, SignerError>> {
        Box::pin(async { Err(SignerError::RawSigningUnsupported) })
    }
}

#[test]
fn transaction_only_signer_refuses_http_signing_through_the_raw_message_boundary() -> TestResult {
    let receiver = bob()?;
    let sender = TransactionOnlySigner(alice(CryptoType::Sr25519)?.account());
    assert_error(
        headers(&sender, Some(&receiver)),
        AuthError::Signing(SignerError::RawSigningUnsupported),
    )?;
    let error = AuthError::Signing(SignerError::RawSigningUnsupported);
    assert_eq!(
        error.to_string(),
        "raw message signing is not supported by this signer"
    );
    assert_eq!(
        error.source().map(ToString::to_string),
        Some(error.to_string())
    );
    Ok(())
}

#[test]
fn full_store_fails_closed_without_evicting_an_accepted_request() -> TestResult {
    let sender = alice(CryptoType::Ed25519)?;
    let receiver = bob()?;
    let store = Arc::new(InMemoryNonceStore::with_config(NonceStoreConfig {
        max_entries: 1,
        max_per_hotkey: 1,
        ..Default::default()
    })?);
    let verifier = Verifier::new(receiver, VerifyConfig::default(), store)?;
    let first = headers(&sender, Some(&receiver))?;
    let second = block_on(sign(
        &sender,
        &Method::POST,
        TARGET,
        BODY,
        Some(&receiver),
        &FixedClock(FIXED_NONCE + 1),
    ))?;
    block_on(verifier.verify(&first, &Method::POST, TARGET, BODY, NOW))?;
    assert_error(
        block_on(verifier.verify(&second, &Method::POST, TARGET, BODY, NOW)),
        AuthError::Store(StoreError::Full),
    )?;
    assert_error(
        block_on(verifier.verify(&first, &Method::POST, TARGET, BODY, NOW)),
        AuthError::ReplayedRequest(REPLAY.into()),
    )?;
    let error = AuthError::Store(StoreError::Full);
    assert_eq!(error.to_string(), "nonce store is full");
    assert_eq!(
        error.source().map(ToString::to_string),
        Some(error.to_string())
    );
    Ok(())
}
