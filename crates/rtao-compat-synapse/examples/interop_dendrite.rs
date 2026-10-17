//! Rust v10 dendrite scenario runner for the bittensor==10.5.0 interop harness.
//!
//! Usage:
//! - `interop_dendrite <port> <python|rust>` runs the scenario matrix against
//!   the axon on 127.0.0.1:<port>, printing one JSON line per check and
//!   exiting non-zero on any failure. `python` expects a bittensor 10.5.0
//!   axon (`py_axon.py`), `rust` the `interop_axon` example.
//! - `interop_dendrite --sign-s1` prints the S1 message and Alice's raw
//!   signature over it (vector S3, verified by Python).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs)]

#[path = "interop/synapses.rs"]
mod synapses;

use std::{
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use futures::StreamExt;
use rtao_btauth::{Clock, ClockError};
use rtao_compat_synapse::{
    BaseSynapse, CallOptions, Dendrite, EMPTY_BODY_HASH, Envelope, PreparedRequest, PyDict,
    Synapse, UuidMode, tensor::Tensor,
};
use rtao_primitives::{AccountId, CryptoType, Uid, axon::AxonTarget};
use rtao_signer::{KeypairSigner, Signer};
use serde_json::{Value, json};
use synapses::*;

const ALICE: &str = "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY";
const BOB: &str = "5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty";
const CHARLIE: &str = "5FLSigC9HGRKVhB9FiEo4Y3koPsNmBmLJbpXg2mp1hXcS59Y";

struct OffsetClock(Duration);
impl Clock for OffsetClock {
    fn now_ns(&self) -> Result<u64, ClockError> {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap() - self.0;
        Ok(u64::try_from(now.as_nanos()).unwrap())
    }
}

struct Raw {
    status: u16,
    headers: http::HeaderMap,
    body: Vec<u8>,
}

impl Raw {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
    fn message(&self) -> String {
        self.json()
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("<none>")
            .to_owned()
    }
    fn header(&self, name: &str) -> Option<String> {
        self.headers
            .get(name)
            .map(|value| String::from_utf8_lossy(value.as_bytes()).into_owned())
    }
}

struct Runner {
    python: bool,
    target: AxonTarget,
    alice: Arc<KeypairSigner>,
    dendrite: Dendrite,
    failures: usize,
}

impl Runner {
    fn check(&mut self, id: &str, ok: bool, detail: impl Into<String>) {
        let detail = detail.into();
        if !ok {
            self.failures += 1;
        }
        println!(
            "{}",
            json!({"id": id, "direction": "rust-dendrite->".to_owned() + if self.python { "python-axon" } else { "rust-axon" }, "pass": ok, "detail": detail})
        );
    }

    fn fresh(&self, offset: Duration) -> Dendrite {
        Dendrite::new(self.alice.clone())
            .unwrap()
            .with_clock(Arc::new(OffsetClock(offset)))
    }

    async fn send(
        &self,
        prepared: &PreparedRequest<impl Synapse>,
        content_type: Option<&str>,
    ) -> Raw {
        self.send_parts(
            prepared.url.as_str(),
            &prepared.headers,
            prepared.body.clone(),
            content_type,
        )
        .await
    }

    async fn send_parts(
        &self,
        url: &str,
        headers: &[(String, String)],
        body: Vec<u8>,
        content_type: Option<&str>,
    ) -> Raw {
        let mut request = self
            .dendrite
            .client()
            .post(url)
            .timeout(Duration::from_secs(12));
        for (name, value) in headers {
            request = request.header(name.as_str(), value.as_str());
        }
        if let Some(content_type) = content_type {
            request = request.header("content-type", content_type);
        }
        let response = request.body(body).send().await.unwrap();
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let body = response.bytes().await.unwrap().to_vec();
        Raw {
            status,
            headers,
            body,
        }
    }

    async fn prepare<S: Synapse>(&self, dendrite: &Dendrite, body: S) -> PreparedRequest<S> {
        dendrite
            .prepare(&self.target, body, &CallOptions::default())
            .await
            .unwrap()
    }
}

fn set_header(prepared: &mut PreparedRequest<impl Synapse>, name: &str, value: Option<String>) {
    prepared.headers.retain(|(header, _)| header != name);
    if let Some(value) = value {
        prepared.headers.push((name.to_owned(), value));
    }
}

fn header<'a>(prepared: &'a PreparedRequest<impl Synapse>, name: &str) -> &'a str {
    prepared
        .headers
        .iter()
        .find(|(header, _)| header == name)
        .map(|(_, value)| value.as_str())
        .unwrap()
}

fn mixed() -> Mixed {
    Mixed {
        i: 7,
        f: 1.5,
        s: "it's \"q\" \u{e9} \u{1F600} \u{200b}\u{a0}\\".into(),
        l: vec!["a".into(), "b'c".into()],
        d: PyDict(vec![
            ("x".into(), vec![0.1, 1e-5, 1e16]),
            ("a".into(), vec![2.0]),
        ]),
        n: None,
        out: None,
    }
}

fn req() -> Req {
    Req {
        rs: "hello".into(),
        ri: 5,
        rl: vec![1, 2],
        rd: PyDict(vec![("k".into(), 1)]),
        rf: 0.25,
        rb: true,
        echo: None,
    }
}

fn is_internal(message: &str) -> bool {
    message.starts_with("Internal Server Error #")
        && message.len() == "Internal Server Error #".len() + 36
}

async fn run(port: u16, python: bool) -> usize {
    let alice = Arc::new(KeypairSigner::from_uri("//Alice", CryptoType::Sr25519).unwrap());
    let bob: AccountId = BOB.parse().unwrap();
    let target = AxonTarget {
        uid: Uid(0),
        hotkey: bob,
        base_url: format!("http://127.0.0.1:{port}").parse().unwrap(),
    };
    let mut runner = Runner {
        python,
        target: target.clone(),
        dendrite: Dendrite::new(alice.clone()).unwrap(),
        alice,
        failures: 0,
    };
    let options = CallOptions::default();
    let dendrite = runner.dendrite.clone();

    // I1: ping, plus the exact success header set.
    let ping = dendrite.call(&target, BaseSynapse {}, &options).await;
    let status = ping.dendrite.as_ref().and_then(|d| d.status_code);
    runner.check("I1", status == Some(200), format!("status {status:?}"));
    let prepared = runner.prepare(&dendrite, BaseSynapse {}).await;
    let raw = runner.send(&prepared, Some("application/json")).await;
    let ok = raw.status == 200
        && raw.header("bt_header_axon_status_code").as_deref() == Some("200")
        && raw.header("bt_header_axon_status_message").as_deref() == Some("Success")
        && raw.header("name").as_deref() == Some("Synapse")
        && raw.header("computed_body_hash").as_deref() == Some(EMPTY_BODY_HASH)
        && raw.header("content-type").as_deref() == Some("application/json")
        && raw.header("bt_header_dendrite_hotkey").as_deref() == Some(ALICE);
    runner.check(
        "I1.headers",
        ok,
        format!("{} {:?}", raw.status, raw.headers),
    );

    // I2: every hashed type.
    let reply = dendrite.call(&target, mixed(), &options).await;
    let status = reply.dendrite.as_ref().and_then(|d| d.status_code);
    let expected_out = format!("7|{}|2", mixed().s);
    runner.check(
        "I2",
        status == Some(200)
            && reply.body.out.as_deref() == Some(expected_out.as_str())
            && reply.body.d == mixed().d,
        format!("status {status:?} out {:?}", reply.body.out),
    );

    // I3: required fields with dummies; then without one dummy.
    let reply = dendrite.call(&target, req(), &options).await;
    let status = reply.dendrite.as_ref().and_then(|d| d.status_code);
    runner.check(
        "I3",
        status == Some(200) && reply.body.echo.as_deref() == Some("hello"),
        format!("status {status:?}"),
    );
    let mut prepared = runner.prepare(&dendrite, req()).await;
    let dummies: Vec<_> = prepared
        .headers
        .iter()
        .filter(|(name, _)| name.starts_with("bt_header_input_obj_"))
        .map(|(name, value)| format!("{name}={value}"))
        .collect();
    runner.check(
        "I3.dummies",
        dummies
            == [
                "bt_header_input_obj_rs=IiI=",
                "bt_header_input_obj_ri=MA==",
                "bt_header_input_obj_rl=W10=",
                "bt_header_input_obj_rd=e30=",
                "bt_header_input_obj_rf=MC4w",
                "bt_header_input_obj_rb=ZmFsc2U=",
            ],
        format!("{dummies:?}"),
    );
    set_header(&mut prepared, "bt_header_input_obj_rb", None);
    let raw = runner.send(&prepared, Some("application/json")).await;
    let ok = if python {
        raw.status == 500 && is_internal(&raw.message())
    } else {
        raw.status == 200
    };
    runner.check(
        "I3.missing-dummy",
        ok,
        format!("{} {}", raw.status, raw.message()),
    );

    let legacy = LegacyHashed {
        a: 1,
        b: 2,
        d: Some(vec!["foobar".into()]),
        ..LegacyHashed::default()
    };
    let prepared = runner.prepare(&dendrite, legacy.clone()).await;
    runner.check(
        "H2/H3.legacy-hash",
        header(&prepared, "computed_body_hash")
            == "ae06397d08f30f75c91395c59f05c62ac3b62b88250eb78b109213258e6ced0c",
        header(&prepared, "computed_body_hash").to_owned(),
    );
    let reply = dendrite.call(&target, legacy, &options).await;
    let status = reply.dendrite.as_ref().and_then(|d| d.status_code);
    runner.check(
        "I2.legacy",
        status == Some(200),
        format!("status {status:?}"),
    );

    // I4: body tampered after signing.
    let mut prepared = runner.prepare(&dendrite, mixed()).await;
    let mut body: Value = serde_json::from_slice(&prepared.body).unwrap();
    body["i"] = json!(8);
    prepared.body = serde_json::to_vec(&body).unwrap();
    let raw = runner.send(&prepared, Some("application/json")).await;
    runner.check(
        "I4",
        raw.status == 500 && is_internal(&raw.message()),
        format!("{} {}", raw.status, raw.message()),
    );

    // I5: computed_body_hash header tampered.
    let mut prepared = runner.prepare(&dendrite, mixed()).await;
    set_header(
        &mut prepared,
        "computed_body_hash",
        Some(EMPTY_BODY_HASH.into()),
    );
    let raw = runner.send(&prepared, Some("application/json")).await;
    let expected = format!(
        "Not Verified with error: Signature mismatch with {}.{ALICE}.{BOB}.{}.{EMPTY_BODY_HASH} and {}",
        header(&prepared, "bt_header_dendrite_nonce"),
        header(&prepared, "bt_header_dendrite_uuid"),
        header(&prepared, "bt_header_dendrite_signature"),
    );
    runner.check(
        "I5",
        raw.status == 401 && raw.message() == expected,
        format!("{} {}", raw.status, raw.message()),
    );

    // I6: nonce 17 s old with timeout 12, fresh uuid.
    let stale = runner.fresh(Duration::from_secs(17));
    let prepared = runner.prepare(&stale, BaseSynapse {}).await;
    let raw = runner.send(&prepared, Some("application/json")).await;
    let message = raw.message();
    let ok = raw.status == 401
        && message.starts_with("Not Verified with error: Nonce is too old: acceptable delta is 16.00 seconds but request was 17.")
        && message.ends_with(" seconds old");
    runner.check("I6", ok, format!("{} {message}", raw.status));

    // I7: identical replay.
    let prepared = runner.prepare(&dendrite, BaseSynapse {}).await;
    let first = runner.send(&prepared, Some("application/json")).await;
    let second = runner.send(&prepared, Some("application/json")).await;
    runner.check(
        "I7",
        first.status == 200
            && second.status == 401
            && second.message()
                == "Not Verified with error: Nonce is too old, a newer one was last processed",
        format!(
            "{} then {} {}",
            first.status,
            second.status,
            second.message()
        ),
    );

    // I8: version header stripped, stale nonce, fresh uuid.
    let stale = runner.fresh(Duration::from_secs(60));
    let mut prepared = runner.prepare(&stale, BaseSynapse {}).await;
    set_header(&mut prepared, "bt_header_dendrite_version", None);
    let raw = runner.send(&prepared, Some("application/json")).await;
    let ok = if python {
        raw.status == 200
    } else {
        raw.status == 401
            && raw
                .message()
                .starts_with("Not Verified with error: Nonce is too old: acceptable delta is 16.00")
    };
    runner.check("I8", ok, format!("{} {}", raw.status, raw.message()));

    // I9: blacklist.
    let reply = dendrite.call(&target, Deny {}, &options).await;
    let axon = reply.axon.clone().unwrap_or_default();
    runner.check(
        "I9",
        axon.status_code == Some(403)
            && axon.status_message.as_deref()
                == Some("Forbidden. Key is blacklisted: denied by test."),
        format!("{:?} {:?}", axon.status_code, axon.status_message),
    );

    // I10: forward sets 404.
    let prepared = runner.prepare(&dendrite, Status404 {}).await;
    let raw = runner.send(&prepared, Some("application/json")).await;
    let body = raw.json();
    runner.check(
        "I10",
        raw.status == 404
            && body.get("name").and_then(Value::as_str) == Some("Status404")
            && body["axon"]["status_code"] == json!(404),
        format!("{} {}", raw.status, String::from_utf8_lossy(&raw.body)),
    );
    let reply = dendrite.call(&target, Status404 {}, &options).await;
    let dendrite_info = reply.dendrite.clone().unwrap_or_default();
    runner.check(
        "I10.merge",
        dendrite_info.status_code == Some(404),
        format!(
            "{:?} {:?}",
            dendrite_info.status_code, dendrite_info.status_message
        ),
    );

    // I11: forward raises.
    let reply = dendrite.call(&target, Boom {}, &options).await;
    let dendrite_info = reply.dendrite.clone().unwrap_or_default();
    runner.check(
        "I11",
        dendrite_info.status_code == Some(500)
            && is_internal(dendrite_info.status_message.as_deref().unwrap_or("")),
        format!(
            "{:?} {:?}",
            dendrite_info.status_code, dendrite_info.status_message
        ),
    );

    // I12: unknown route.
    let prepared = runner.prepare(&dendrite, BaseSynapse {}).await;
    let url = prepared.url.as_str().replace("/Synapse", "/Nope");
    let raw = runner
        .send_parts(
            &url,
            &prepared.headers,
            prepared.body.clone(),
            Some("application/json"),
        )
        .await;
    let expected = "Synapse name 'Nope' not found. Available synapses ['Synapse', 'Mixed', 'Req', 'LegacyHashed', 'Tens', 'Deny', 'Status404', 'Boom', 'Slow', 'NanResp', 'WhoAmI', 'Echo', 'NanReq']";
    runner.check(
        "I12",
        raw.status == 404 && raw.message() == expected,
        format!("{} {}", raw.status, raw.message()),
    );

    // I13: invalid JSON.
    let prepared = runner.prepare(&dendrite, BaseSynapse {}).await;
    let raw = runner
        .send_parts(
            prepared.url.as_str(),
            &prepared.headers,
            b"{".to_vec(),
            Some("application/json"),
        )
        .await;
    runner.check(
        "I13",
        raw.status == 422 && raw.json().get("detail").is_some_and(Value::is_array),
        format!("{} {}", raw.status, String::from_utf8_lossy(&raw.body)),
    );

    // I14: streaming.
    let (stream, done) = dendrite.call_stream(
        &target,
        Echo {
            text: "hi".into(),
            completion: String::new(),
        },
        &options,
    );
    let chunks: Vec<_> = stream.collect().await;
    let text: Vec<u8> = chunks
        .iter()
        .flat_map(|chunk| chunk.as_ref().unwrap().to_vec())
        .collect();
    let last = done.await.unwrap();
    let status = last.dendrite.as_ref().and_then(|d| d.status_code);
    runner.check(
        "I14",
        text == b"tok\ntok\ntok\n" && status == Some(200),
        format!("{:?} {status:?}", String::from_utf8_lossy(&text)),
    );
    let prepared = runner.prepare(&dendrite, Echo::default()).await;
    let raw = runner.send(&prepared, Some("application/json")).await;
    runner.check(
        "I14.headers",
        raw.status == 200
            && raw.header("content-type").as_deref() == Some("text/event-stream")
            && raw.header("bt_header_axon_status_code").as_deref() == Some("200")
            && raw.header("computed_body_hash").as_deref() == Some(EMPTY_BODY_HASH),
        format!("{:?}", raw.headers),
    );

    // I15: total deadline.
    let short = CallOptions {
        timeout: Duration::from_secs(2),
        ..CallOptions::default()
    };
    let started = Instant::now();
    let reply = dendrite.call(&target, Slow { delay: 3.0 }, &short).await;
    let dendrite_info = reply.dendrite.clone().unwrap_or_default();
    runner.check(
        "I15",
        dendrite_info.status_code == Some(408)
            && dendrite_info.status_message.as_deref() == Some("Request timeout after 2.0 seconds")
            && started.elapsed() < Duration::from_millis(2900),
        format!(
            "{:?} {:?}",
            dendrite_info.status_code, dendrite_info.status_message
        ),
    );

    // I16: tensor round trip.
    let data: Vec<u8> = [1.5f32, 2.0, 3.0, 4.0]
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    let tensor = Tensor::from_raw("float32", &[2, 2], &data).unwrap();
    let reply = dendrite
        .call(
            &target,
            Tens {
                t: Some(tensor.clone()),
                total: None,
            },
            &options,
        )
        .await;
    let status = reply.dendrite.as_ref().and_then(|d| d.status_code);
    runner.check(
        "I16",
        status == Some(200)
            && reply.body.t.as_ref() == Some(&tensor)
            && reply.body.total == Some(10.5),
        format!("{status:?} {:?}", reply.body.total),
    );

    // I17: eight concurrent calls sharing one uuid.
    let shared = runner.fresh(Duration::ZERO);
    let prepared: Vec<_> =
        futures::future::join_all((0..8).map(|_| runner.prepare(&shared, BaseSynapse {}))).await;
    let results = futures::future::join_all(
        prepared
            .iter()
            .rev()
            .map(|p| runner.send(p, Some("application/json"))),
    )
    .await;
    let accepted = results.iter().filter(|raw| raw.status == 200).count();
    let rejected = results
        .iter()
        .filter(|raw| {
            raw.status == 401
                && raw.message()
                    == "Not Verified with error: Nonce is too old, a newer one was last processed"
        })
        .count();
    runner.check(
        "I17",
        accepted >= 1 && accepted + rejected == 8,
        format!("accepted {accepted} rejected {rejected} (sent newest first)"),
    );

    // I17.order: with one uuid, exactly the out-of-order arrival is refused.
    let shared = runner.fresh(Duration::ZERO);
    let first = runner.prepare(&shared, BaseSynapse {}).await;
    let second = runner.prepare(&shared, BaseSynapse {}).await;
    let third = runner.prepare(&shared, BaseSynapse {}).await;
    let nonces: Vec<u64> = [&first, &second, &third]
        .iter()
        .map(|p| header(p, "bt_header_dendrite_nonce").parse().unwrap())
        .collect();
    let mut order = Vec::new();
    for prepared in [&second, &first, &third] {
        order.push(runner.send(prepared, Some("application/json")).await);
    }
    let statuses: Vec<u16> = order.iter().map(|raw| raw.status).collect();
    runner.check(
        "I17.order",
        nonces.windows(2).all(|pair| pair[0] < pair[1])
            && statuses == [200, 401, 200]
            && order[1].message()
                == "Not Verified with error: Nonce is too old, a newer one was last processed",
        format!("{nonces:?} {statuses:?} {}", order[1].message()),
    );

    // M1: a Python dendrite's bare NaN/Infinity tokens, re-signed over
    // Python's hash of the non-finite values.
    let prepared = runner.prepare(&dendrite, NanReq::default()).await;
    let text = String::from_utf8(prepared.body.clone()).unwrap().replace(
        r#""v": 0.0, "vs": []"#,
        r#""v": NaN, "vs": [Infinity, -Infinity]"#,
    );
    let non_finite = Envelope::new(NanReq {
        v: f64::NAN,
        vs: vec![f64::INFINITY, f64::NEG_INFINITY],
        seen: None,
    });
    let hash = non_finite.body_hash().unwrap();
    let message = rtao_compat_synapse::v10_message(
        &header(&prepared, "bt_header_dendrite_nonce")
            .parse()
            .unwrap(),
        ALICE,
        BOB,
        Some(header(&prepared, "bt_header_dendrite_uuid")),
        Some(&hash),
    );
    let signature = runner.alice.sign_raw(message.as_bytes()).await.unwrap();
    let mut headers = prepared.headers.clone();
    headers
        .retain(|(name, _)| name != "computed_body_hash" && name != "bt_header_dendrite_signature");
    headers.push(("computed_body_hash".into(), hash));
    headers.push((
        "bt_header_dendrite_signature".into(),
        format!("0x{}", hex::encode(signature)),
    ));
    let raw = runner
        .send_parts(
            prepared.url.as_str(),
            &headers,
            text.into_bytes(),
            Some("application/json"),
        )
        .await;
    runner.check(
        "M1.nan-request",
        raw.status == 200 && raw.json()["seen"] == "(nan, [inf, -inf])",
        format!("{} {}", raw.status, String::from_utf8_lossy(&raw.body)),
    );

    // I19: stale capture replayed with `timeout: nan`.
    let stale = runner.fresh(Duration::from_secs(3600));
    let mut prepared = runner.prepare(&stale, BaseSynapse {}).await;
    set_header(&mut prepared, "timeout", Some("nan".into()));
    let raw = runner.send(&prepared, Some("application/json")).await;
    let ok = if python {
        raw.status == 200
    } else {
        raw.status == 401
            && raw
                .message()
                .starts_with("Not Verified with error: Invalid timeout: nan")
    };
    runner.check("I19", ok, format!("{} {}", raw.status, raw.message()));

    // I20: body dendrite.hotkey differs from the signed header.
    let mut prepared = runner.prepare(&dendrite, WhoAmI::default()).await;
    let mut body: Value = serde_json::from_slice(&prepared.body).unwrap();
    body["dendrite"]["hotkey"] = json!(CHARLIE);
    prepared.body = serde_json::to_vec(&body).unwrap();
    let raw = runner.send(&prepared, Some("application/json")).await;
    let seen = raw.json()["seen"].as_str().unwrap_or("").to_owned();
    let expected = if python { CHARLIE } else { ALICE };
    runner.check(
        "I20",
        raw.status == 200 && seen == expected,
        format!("{} seen {seen}", raw.status),
    );

    // I21: text/plain body. FastAPI 0.143 hands the bytes to the synapse
    // validator, which raises: 500 (the spec predicted 422).
    let prepared = runner.prepare(&dendrite, BaseSynapse {}).await;
    let raw = runner.send(&prepared, Some("text/plain")).await;
    runner.check(
        "I21",
        raw.status == 500 && is_internal(&raw.message()),
        format!("{} {}", raw.status, raw.message()),
    );
    let prepared = runner.prepare(&dendrite, BaseSynapse {}).await;
    let raw = runner.send(&prepared, None).await;
    runner.check(
        "I21.no-content-type",
        raw.status == 500 && is_internal(&raw.message()),
        format!("{} {}", raw.status, raw.message()),
    );

    // I22: empty body.
    let prepared = runner.prepare(&dendrite, BaseSynapse {}).await;
    let raw = runner
        .send_parts(
            prepared.url.as_str(),
            &prepared.headers,
            Vec::new(),
            Some("application/json"),
        )
        .await;
    runner.check(
        "I22",
        raw.status == 500 && is_internal(&raw.message()),
        format!("{} {}", raw.status, raw.message()),
    );

    // I23: NaN in a response field.
    let prepared = runner.prepare(&dendrite, NanResp::default()).await;
    let raw = runner.send(&prepared, Some("application/json")).await;
    let body = String::from_utf8_lossy(&raw.body).into_owned();
    runner.check(
        "I23",
        (raw.status == 200 && raw.json()["v"].is_null()) || (python && raw.status == 500),
        format!("{} {body}", raw.status),
    );

    // I24 / S4, S7-S9, S11.
    let prepared = runner.prepare(&dendrite, BaseSynapse {}).await;
    let mut zero = prepared.clone();
    let zeros = format!("0x{}", "00".repeat(64));
    set_header(
        &mut zero,
        "bt_header_dendrite_signature",
        Some(zeros.clone()),
    );
    let raw = runner.send(&zero, Some("application/json")).await;
    let expected = format!(
        "Not Verified with error: Signature mismatch with {} and {zeros}",
        prepared.message
    );
    runner.check(
        "S4",
        raw.status == 401 && raw.message() == expected,
        format!("{} {}", raw.status, raw.message()),
    );

    let mut bare = runner.prepare(&dendrite, BaseSynapse {}).await;
    set_header(
        &mut bare,
        "bt_header_dendrite_signature",
        Some("00".repeat(64)),
    );
    let raw = runner.send(&bare, Some("application/json")).await;
    runner.check(
        "S7",
        raw.status == 401
            && raw.message()
                == "Not Verified with error: Invalid signature format. Expected hex string.",
        format!("{} {}", raw.status, raw.message()),
    );
    let mut short_sig = runner.prepare(&dendrite, BaseSynapse {}).await;
    set_header(
        &mut short_sig,
        "bt_header_dendrite_signature",
        Some(format!("0x{}", "00".repeat(63))),
    );
    let raw = runner.send(&short_sig, Some("application/json")).await;
    runner.check(
        "S8",
        raw.status == 401 && raw.message() == "Not Verified with error: Invalid SR25519 signature.",
        format!("{} {}", raw.status, raw.message()),
    );
    let mut doubled = runner.prepare(&dendrite, BaseSynapse {}).await;
    let signature = header(&doubled, "bt_header_dendrite_signature").to_owned();
    set_header(
        &mut doubled,
        "bt_header_dendrite_signature",
        Some(format!("0x{signature}")),
    );
    let raw = runner.send(&doubled, Some("application/json")).await;
    runner.check(
        "S9",
        raw.status == 200,
        format!("{} {}", raw.status, raw.message()),
    );
    let mut plus = runner.prepare(&dendrite, BaseSynapse {}).await;
    let nonce = header(&plus, "bt_header_dendrite_nonce").to_owned();
    set_header(
        &mut plus,
        "bt_header_dendrite_nonce",
        Some(format!("+{nonce}")),
    );
    let raw = runner.send(&plus, Some("application/json")).await;
    runner.check(
        "S11",
        raw.status == 200,
        format!("{} {}", raw.status, raw.message()),
    );
    let mut missing = runner.prepare(&dendrite, BaseSynapse {}).await;
    set_header(&mut missing, "bt_header_dendrite_signature", None);
    let raw = runner.send(&missing, Some("application/json")).await;
    runner.check(
        "S10",
        raw.status == 401 && raw.message() == "Not Verified with error: Missing Signature",
        format!("{} {}", raw.status, raw.message()),
    );

    // Per-request uuids never collide under concurrency.
    let per_request = CallOptions {
        uuid_mode: UuidMode::PerRequest,
        ..CallOptions::default()
    };
    let targets = vec![target.clone(); 8];
    let replies = dendrite
        .forward(&targets, BaseSynapse {}, &per_request, 8)
        .await;
    let ok = replies
        .iter()
        .all(|reply| reply.dendrite.as_ref().and_then(|d| d.status_code) == Some(200));
    runner.check(
        "I17.per-request-uuid",
        ok,
        format!("{} replies", replies.len()),
    );

    let _ = Envelope::new(BaseSynapse {});
    runner.failures
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--sign-s1") {
        let message = format!(
            "1700000000000000000.{ALICE}.{BOB}.5ecbd69c-1cec-11ee-b0dc-e29ce36fec1a.{EMPTY_BODY_HASH}"
        );
        let alice = KeypairSigner::from_uri("//Alice", CryptoType::Sr25519).unwrap();
        let signature = alice.sign_raw(message.as_bytes()).await.unwrap();
        println!(
            "{}",
            json!({"message": message, "signature": format!("0x{}", hex::encode(signature))})
        );
        return;
    }
    let port: u16 = args
        .get(1)
        .and_then(|port| port.parse().ok())
        .expect("usage: interop_dendrite <port> <python|rust>");
    let python = match args.get(2).map(String::as_str) {
        Some("python") => true,
        Some("rust") => false,
        _ => panic!("peer must be python or rust"),
    };
    let failures = run(port, python).await;
    println!("{}", json!({"summary": true, "failures": failures}));
    std::process::exit(i32::from(failures > 0));
}
