// Derived from RaoFoundation/subtensor sdk/python/bittensor/vault/server.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Scripted loopback peers and offline vectors; no browser, camera or phone runs.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::{
    error::Error,
    sync::{Arc, OnceLock},
    time::Duration,
};

use base64::Engine;
use futures::{SinkExt, StreamExt};
use parity_scale_codec::Decode;
use serde_json::{Value, json};
use tokio::{
    net::{TcpListener, TcpStream},
    time::timeout,
};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async,
    tungstenite::{Message, client::IntoClientRequest, protocol::frame::coding::CloseCode},
};

use super::*;
use crate::{
    CryptoType, KeypairSigner, SignPurpose, Signer, SignerPayloadJson, SigningContext,
    SigningRequest,
};

type TestResult = Result<(), Box<dyn Error>>;
type Browser = WebSocketStream<MaybeTlsStream<TcpStream>>;
fn timeouts() -> VaultRequestTimeouts {
    VaultRequestTimeouts {
        connect: Duration::from_secs(2),
        answer: Duration::from_secs(2),
    }
}
async fn server() -> Result<VaultSessionServer, VaultPageError> {
    let server = VaultSessionServer::new(VaultServerOptions::default())?;
    server.start().await?;
    Ok(server)
}
async fn browser(server: &VaultSessionServer) -> Result<Browser, Box<dyn Error>> {
    let (mut socket, _) = connect_async(server.ws_url()).await?;
    socket
        .send(Message::Text(
            json!({"role":"vault","session":server.session_id()})
                .to_string()
                .into(),
        ))
        .await?;
    Ok(socket)
}
async fn read(socket: &mut Browser) -> Result<Value, Box<dyn Error>> {
    timeout(Duration::from_secs(15), async {
        loop {
            match socket.next().await {
                Some(Ok(Message::Text(text))) => return Ok(serde_json::from_str(&text)?),
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                other => return Err(format!("unexpected Vault fixture response: {other:?}").into()),
            }
        }
    })
    .await?
}
async fn send(socket: &mut Browser, value: Value) -> Result<(), Box<dyn Error>> {
    socket.send(Message::Text(value.to_string().into())).await?;
    Ok(())
}

#[test]
fn construction_is_offline_and_raw_signing_is_refused() {
    let software = KeypairSigner::from_uri("//Alice", CryptoType::Sr25519).unwrap();
    let signer = VaultSigner::new(&software.account().to_ss58(2)).unwrap();
    assert_eq!(signer.account(), software.account());
    assert_eq!(signer.ss58_address(), software.account().to_ss58(2));
    assert_eq!(signer.crypto_type(), CryptoType::Sr25519);
    assert_eq!(signer.metadata_constants(), Some((9, "TAO".into())));
    assert_eq!(
        signer.caps(),
        crate::SignerCaps::METADATA_HASH
            | crate::SignerCaps::UNSIGNED_TX
            | crate::SignerCaps::OUT_OF_BAND
    );
    assert_eq!(
        futures::executor::block_on(signer.sign_raw(b"message")),
        Err(crate::SignerError::RawSigningUnsupported)
    );
    assert!(VaultSigner::new("invalid address").is_err());
    assert!(
        VaultSessionServer::new(VaultServerOptions {
            address: "0.0.0.0:0".parse().unwrap(),
            ..Default::default()
        })
        .is_err()
    );
    assert_eq!(
        VaultScanOptions::default().timeouts.answer,
        Duration::from_secs(600)
    );
}

#[test]
fn address_scan_parser_matches_substrate_and_bare_spelling_rules() {
    for (input, address, genesis) in [
        ("5Bare", "5Bare", None),
        (" 5Bare ", " 5Bare ", None),
        ("substrate:5Alice:abcd", "5Alice", Some("0xabcd")),
        ("substrate:5Alice:0xabcd:ignored", "5Alice", Some("0xabcd")),
        ("substrate:5Alice:", "5Alice", None),
        ("substrate:5Alice", "5Alice", None),
        ("substrate:", "", None),
        ("Substrate:5Alice:abcd", "Substrate:5Alice:abcd", None),
        ("substrate:5Alice:0XAB", "5Alice", Some("0x0XAB")),
    ] {
        assert_eq!(
            parse_vault_address(input),
            (address.to_owned(), genesis.map(str::to_owned))
        );
    }
}

#[test]
fn returned_signatures_authenticate_payload_identity_and_version() {
    let payload = b"prepared exact payload";
    for crypto in [CryptoType::Ed25519, CryptoType::Sr25519] {
        let software = KeypairSigner::from_uri("//Alice", crypto).unwrap();
        let signer = VaultSigner::with_options(
            &software.account().to_string(),
            VaultOptions {
                crypto_type: crypto,
                ..Default::default()
            },
        )
        .unwrap();
        let raw = futures::executor::block_on(software.sign_raw(payload)).unwrap();
        let hex = hex::encode(raw);
        for scan in [
            hex.clone(),
            format!(" \n0x{hex}\t"),
            hex.as_bytes()
                .chunks(2)
                .map(|pair| std::str::from_utf8(pair).unwrap())
                .collect::<Vec<_>>()
                .join(" \t"),
        ] {
            let signature = signer.validate_signature(&scan, payload).unwrap();
            assert_eq!(signature.bytes, raw);
            assert_eq!(signature.crypto, crypto);
        }
        let versioned = format!("{:02x}{hex}", u8::from(crypto));
        assert_eq!(
            signer
                .validate_signature(&versioned, payload)
                .unwrap()
                .crypto,
            crypto
        );
        assert!(
            signer
                .validate_signature(&versioned, b"different payload")
                .is_err()
        );
        assert!(
            signer
                .validate_signature(&format!("02{hex}"), payload)
                .is_err()
        );
        assert!(
            signer
                .validate_signature(&format!("0X{hex}"), payload)
                .is_err()
        );
        assert!(signer.validate_signature("a a", payload).is_err());
        assert!(signer.validate_signature("not hex", payload).is_err());
        assert!(signer.validate_signature("", payload).is_err());
        let other = KeypairSigner::from_uri("//Bob", crypto).unwrap();
        let other_signature = futures::executor::block_on(other.sign_raw(payload)).unwrap();
        assert!(
            signer
                .validate_signature(&hex::encode(other_signature), payload)
                .is_err()
        );

        // Pinned Python delegates to the core verifier, including its <Bytes>
        // retry. This is protocol compatibility, not strict chain-payload proof.
        let wrapped = [b"<Bytes>".as_slice(), payload, b"</Bytes>"].concat();
        let wrapped_signature = futures::executor::block_on(software.sign_raw(&wrapped)).unwrap();
        assert!(
            signer
                .validate_signature(&hex::encode(wrapped_signature), payload)
                .is_ok()
        );
        assert!(
            !rtao_core::keys::verify_strict(
                payload,
                &wrapped_signature,
                signer.ss58_address(),
                u8::from(crypto)
            )
            .unwrap()
        );
    }
    let ed = KeypairSigner::from_uri("//Alice", CryptoType::Ed25519).unwrap();
    let signer = VaultSigner::new(&ed.account().to_string()).unwrap();
    let signature = futures::executor::block_on(ed.sign_raw(payload)).unwrap();
    assert_eq!(signer.crypto_type(), CryptoType::Sr25519);
    assert_eq!(
        signer
            .validate_signature(&format!("00{}", hex::encode(signature)), payload)
            .unwrap()
            .crypto,
        CryptoType::Ed25519
    );
    assert!(
        signer
            .validate_signature(&hex::encode(signature), payload)
            .is_err()
    );
}

#[test]
fn era_deadlines_follow_normalized_and_scale_encoded_periods() {
    for era in [
        None,
        Some(json!(null)),
        Some(json!("00")),
        Some(json!("0x00")),
        Some(json!({"period":0})),
    ] {
        assert_eq!(signer::era_seconds(era.as_ref()).unwrap(), None);
    }
    assert_eq!(
        signer::era_seconds(Some(&json!({"period":8,"current":123}))).unwrap(),
        Some(96.0)
    );
    assert_eq!(
        signer::era_seconds(Some(&json!("0x0200"))).unwrap(),
        Some(96.0)
    );
    assert_eq!(
        signer::era_seconds(Some(&json!("0x0500"))).unwrap(),
        Some(768.0)
    );
    assert_eq!(
        signer::era_seconds(Some(&json!("0x0f00"))).unwrap(),
        Some(786432.0)
    );
    for era in [json!("0x02"), json!("0x0000"), json!("0xf201"), json!(true)] {
        assert!(signer::era_seconds(Some(&era)).is_err());
    }
}

fn decode_svg(data_uri: &str) -> Vec<u8> {
    let encoded = data_uri.strip_prefix("data:image/svg+xml;base64,").unwrap();
    let svg = String::from_utf8(
        base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .unwrap(),
    )
    .unwrap();
    // Rasterize the actual emitted SVG geometry, then let an independent QR
    // decoder recover binary bytes. No encoder module matrix is reused here.
    let size: usize = svg
        .split("viewBox=\"0 0 ")
        .nth(1)
        .unwrap()
        .split(' ')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    let scale = 4usize;
    let width = size * scale;
    let mut raster = vec![255u8; width * width];
    let path = svg
        .split(" d=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();
    for module in path.split('M').filter(|part| !part.is_empty()) {
        let (x, y) = module
            .strip_suffix("h1v1h-1z")
            .unwrap()
            .split_once(',')
            .unwrap();
        let x: usize = x.parse().unwrap();
        let y: usize = y.parse().unwrap();
        assert!(x >= 4 && x < size - 4 && y >= 4 && y < size - 4);
        for dy in 0..scale {
            for dx in 0..scale {
                raster[(y * scale + dy) * width + x * scale + dx] = 0;
            }
        }
    }
    let mut image =
        rqrr::PreparedImage::prepare_from_greyscale(width, width, |x, y| raster[y * width + x]);
    let mut grids = image.detect_grids();
    assert_eq!(grids.len(), 1);
    let mut decoded = Vec::new();
    grids.remove(0).decode_to(&mut decoded).unwrap();
    decoded
}

#[test]
fn rendered_svg_decodes_exact_binary_and_text_like_bytes() {
    for frame in [
        vec![],
        b"0123456789012345".to_vec(),
        b"ABCDEF123456".to_vec(),
        vec![0, 0, 1, 0, 0, 0x53, 1, 6, 0xff, 0x80],
        (0..1029).map(|index| index as u8).collect(),
    ] {
        let uri = svg_data_uri(&frame).unwrap();
        assert_eq!(decode_svg(&uri), frame);
    }
    assert!(svg_data_uri(&vec![0; 2332]).is_err());
}

#[tokio::test]
async fn page_serves_exact_pinned_assets_and_refuses_foreign_origin() -> TestResult {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let server = server().await?;
    for (path, expected, mime) in [
        (
            "/",
            include_bytes!("../../../../vendor/upstream/vault/static/vault.html").as_slice(),
            "text/html; charset=utf-8",
        ),
        (
            "/index.html",
            include_bytes!("../../../../vendor/upstream/vault/static/vault.html").as_slice(),
            "text/html; charset=utf-8",
        ),
        (
            "/bittensor-chain-specs.png",
            include_bytes!("../../../../vendor/upstream/vault/static/bittensor-chain-specs.png")
                .as_slice(),
            "image/png",
        ),
    ] {
        let mut stream = TcpStream::connect(server.address()).await?;
        stream
            .write_all(
                format!(
                    "GET {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
                    server.address()
                )
                .as_bytes(),
            )
            .await?;
        let mut response = Vec::new();
        timeout(Duration::from_secs(2), stream.read_to_end(&mut response)).await??;
        let split = response
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .unwrap();
        let headers = std::str::from_utf8(&response[..split])?;
        assert!(headers.starts_with("HTTP/1.1 200"));
        assert!(headers.contains(mime));
        assert_eq!(&response[split + 4..], expected);
    }
    let mut request = server.ws_url().into_client_request()?;
    request
        .headers_mut()
        .insert("Origin", "https://unrelated.example".parse()?);
    assert!(tokio_tungstenite::connect_async(request).await.is_err());
    server.stop().await?;
    Ok(())
}

#[tokio::test]
async fn stale_session_is_closed_with_policy_code() -> TestResult {
    let server = server().await?;
    let (mut socket, _) = connect_async(server.ws_url()).await?;
    send(&mut socket, json!({"role":"vault","session":"stale"})).await?;
    let response = timeout(Duration::from_secs(2), socket.next())
        .await?
        .unwrap()?;
    let Message::Close(Some(frame)) = response else {
        panic!("expected stale-session close");
    };
    assert_eq!(frame.code, CloseCode::Policy);
    assert_eq!(frame.reason, "stale vault session");
    server.stop().await?;
    Ok(())
}

#[tokio::test]
async fn sequential_requests_result_and_reload_use_the_pinned_page_messages() -> TestResult {
    let server = Arc::new(server().await?);
    let mut first = browser(&server).await?;
    let request_server = server.clone();
    let request = tokio::spawn(async move {
        request_server
            .request_signature(
                vec!["svg".into()],
                vec!["000102".into()],
                json!({"text":"test"}),
                timeouts(),
            )
            .await
    });
    let original = read(&mut first).await?;
    assert_eq!(
        original,
        json!({"type":"sign-request","frames":["svg"],"framesHex":["000102"],"summary":{"text":"test"}})
    );
    let mut second = browser(&server).await?;
    assert_eq!(read(&mut second).await?, original);
    let replaced = timeout(Duration::from_secs(2), first.next())
        .await?
        .unwrap()?;
    let Message::Close(Some(frame)) = replaced else {
        panic!("expected replaced-session close");
    };
    assert_eq!(frame.code, CloseCode::Restart);
    send(
        &mut second,
        json!({"type":"signature","signature":"0xanswer"}),
    )
    .await?;
    assert_eq!(request.await??, "0xanswer");

    let request_server = server.clone();
    let request = tokio::spawn(async move {
        request_server
            .request_scan("show address", timeouts())
            .await
    });
    assert_eq!(
        read(&mut second).await?,
        json!({"type":"scan-request","prompt":"show address"})
    );
    send(
        &mut second,
        json!({"type":"signature","signature":"stale signature"}),
    )
    .await?;
    send(
        &mut second,
        json!({"type":"scanned","text":"substrate:5Alice:abcd"}),
    )
    .await?;
    assert_eq!(request.await??, "substrate:5Alice:abcd");
    server.report_result(true).await?;
    assert_eq!(
        read(&mut second).await?,
        json!({"type":"result","success":true})
    );
    server.warm_up(Duration::from_secs(2)).await?;
    assert_eq!(read(&mut second).await?, json!({"type":"warm-up"}));
    server.stop().await?;
    Ok(())
}

#[tokio::test]
async fn timeout_cancellation_page_errors_and_stop_clear_the_pending_request() -> TestResult {
    let server = Arc::new(server().await?);
    assert!(
        server
            .request_scan(
                "no page",
                VaultRequestTimeouts {
                    connect: Duration::from_millis(10),
                    ..timeouts()
                }
            )
            .await
            .is_err()
    );
    let mut socket = browser(&server).await?;
    let request_server = server.clone();
    let request =
        tokio::spawn(async move { request_server.request_scan("cancel this", timeouts()).await });
    let _ = read(&mut socket).await?;
    request.abort();
    assert!(request.await.is_err());
    // A timed-out/canceled page is retired because its protocol has no ids.
    // Its late reply must never become the next scan's answer.
    let _ = send(&mut socket, json!({"type":"scanned","text":"stale answer"})).await;
    let mut reloaded = browser(&server).await?;
    assert!(
        timeout(Duration::from_millis(30), reloaded.next())
            .await
            .is_err()
    );
    let request_server = server.clone();
    let request =
        tokio::spawn(async move { request_server.request_scan("page error", timeouts()).await });
    assert_eq!(read(&mut reloaded).await?["prompt"], "page error");
    send(
        &mut reloaded,
        json!({"type":"error","message":"camera denied"}),
    )
    .await?;
    assert_eq!(request.await?.unwrap_err().to_string(), "camera denied");
    let mut reloaded = browser(&server).await?;
    let request_server = server.clone();
    let request =
        tokio::spawn(async move { request_server.request_scan("stop this", timeouts()).await });
    let _ = read(&mut reloaded).await?;
    let address = server.address();
    server.stop().await?;
    assert!(request.await?.is_err());
    assert!(TcpStream::connect(address).await.is_err());
    Ok(())
}

#[tokio::test]
async fn dropping_owned_server_releases_listener_and_connections() -> TestResult {
    let server = server().await?;
    let address = server.address();
    let _socket = browser(&server).await?;
    drop(server);
    tokio::task::yield_now().await;
    let listener = TcpListener::bind(address).await?;
    drop(listener);
    Ok(())
}

#[tokio::test]
async fn address_import_stops_its_session_after_the_scan() -> TestResult {
    let (started, url) = tokio::sync::oneshot::channel();
    let started = Arc::new(std::sync::Mutex::new(Some(started)));
    let options = VaultScanOptions {
        on_status: Some(Arc::new(move |message| {
            if let Some(sender) = started.lock().unwrap().take() {
                let url = message
                    .split_whitespace()
                    .find(|part| part.starts_with("http://"))
                    .unwrap();
                let _ = sender.send(url.to_owned());
            }
        })),
        timeouts: timeouts(),
        ..Default::default()
    };
    let phone = async {
        let url = timeout(Duration::from_secs(2), url).await??;
        let (authority, session) = url
            .strip_prefix("http://")
            .unwrap()
            .split_once("/?session=")
            .unwrap();
        let (mut socket, _) = connect_async(format!("ws://{authority}/ws")).await?;
        send(&mut socket, json!({"role":"vault","session":session})).await?;
        assert_eq!(read(&mut socket).await?["type"], "scan-request");
        send(
            &mut socket,
            json!({"type":"scanned","text":"substrate:5Public:abcd"}),
        )
        .await?;
        Ok::<_, Box<dyn Error>>(authority.to_owned())
    };
    let (scan, address) = tokio::join!(scan_vault_address(&options), phone);
    assert_eq!(scan?, ("5Public".to_owned(), Some("0xabcd".to_owned())));
    assert!(TcpStream::connect(address?).await.is_err());
    Ok(())
}

#[tokio::test]
async fn dropping_signer_stops_its_page_even_with_an_external_server_handle() -> TestResult {
    let software = KeypairSigner::from_uri("//Alice", CryptoType::Sr25519)?;
    let signer = VaultSigner::new(&software.account().to_string())?;
    let handle = signer.session_server().await?;
    let address = handle.address();
    drop(signer);
    tokio::task::yield_now().await;
    assert!(TcpStream::connect(address).await.is_err());
    assert!(handle.request_scan("closed", timeouts()).await.is_err());
    Ok(())
}

fn context() -> SigningContext {
    static CONTEXT: OnceLock<SigningContext> = OnceLock::new();
    CONTEXT
        .get_or_init(|| {
            let golden: Value =
                serde_json::from_str(include_str!("../../../../vendor/upstream/golden.json"))
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
                chain_info: rtao_core::digest::ChainInfo {
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

#[tokio::test]
async fn proof_signer_embeds_existing_proof_vector_and_verifies_scripted_signature() -> TestResult {
    let software = KeypairSigner::from_uri("//Alice", CryptoType::Sr25519)?;
    let signer = VaultSigner::with_options(
        &software.account().to_string(),
        VaultOptions {
            timeouts: VaultRequestTimeouts {
                connect: Duration::from_secs(5),
                answer: Duration::from_secs(20),
            },
            ..Default::default()
        },
    )?;
    let server = signer.session_server().await?;
    let mut socket = browser(&server).await?;
    let vector: Value = serde_json::from_str(include_str!(
        "../../../../vendor/upstream/ledger_proof_vector.json"
    ))?;
    let decode = |name: &str| hex::decode(vector[name].as_str().unwrap()).unwrap();
    let call = decode("call_data_hex");
    let extra = decode("included_in_extrinsic_hex");
    let additional = decode("included_in_signed_data_hex");
    let proof = decode("rust_proof_hex");
    let payload = [&*call, &*extra, &*additional].concat();
    let context = context();
    let json = SignerPayloadJson::from_iter([
        ("era".into(), json!("0x00")),
        ("nonce".into(), json!("0x00000000")),
    ]);
    let request = SigningRequest {
        call: &call,
        included_in_extrinsic: &extra,
        included_in_signed_data: &additional,
        payload: &payload,
        payload_json: &json,
        context: &context,
        purpose: SignPurpose::ShieldInner,
    };
    let phone = async {
        let message = read(&mut socket).await?;
        assert_eq!(message["type"], "sign-request");
        assert_eq!(message["summary"]["hashed"], false);
        assert_eq!(message["summary"]["nonce"], 0);
        assert_eq!(message["summary"]["specVersion"], 461);
        assert_eq!(message["summary"]["deadlineSeconds"], Value::Null);
        assert_eq!(
            message["summary"]["stage"],
            "scan 1 of 2 — the shielded transaction"
        );
        let frames: Vec<Vec<u8>> = message["framesHex"]
            .as_array()
            .unwrap()
            .iter()
            .map(|hex| hex::decode(hex.as_str().unwrap()).unwrap())
            .collect();
        let actual: Vec<u8> = frames
            .iter()
            .flat_map(|frame| &frame[5..])
            .copied()
            .collect();
        assert_eq!(&actual[..3], &[0x53, 1, 6]);
        assert_eq!(&actual[3..35], signer.public_key());
        assert_eq!(&actual[35..35 + proof.len()], proof);
        assert_eq!(
            &actual[35 + proof.len()..35 + proof.len() + 1],
            &[(call.len() as u8) << 2]
        );
        assert_eq!(&actual[36 + proof.len()..actual.len() - 32], payload);
        assert_eq!(&actual[actual.len() - 32..], context.genesis_hash);
        for (uri, frame) in message["frames"].as_array().unwrap().iter().zip(&frames) {
            assert_eq!(decode_svg(uri.as_str().unwrap()), *frame);
        }
        let signature = software.sign_raw(&payload).await?;
        send(
            &mut socket,
            json!({"type":"signature","signature":format!("01{}",hex::encode(signature))}),
        )
        .await?;
        Ok::<_, Box<dyn Error>>(signature)
    };
    let (result, expected) = tokio::join!(signer.sign_unsigned_extrinsic(&request), phone);
    assert_eq!(result?.bytes, expected?);
    signer.close().await?;
    Ok(())
}

#[tokio::test]
async fn legacy_shield_stages_follow_purpose_and_reuse_one_session() -> TestResult {
    let software = KeypairSigner::from_uri("//Alice", CryptoType::Ed25519)?;
    let signer = VaultSigner::with_options(
        &software.account().to_string(),
        VaultOptions {
            crypto_type: CryptoType::Ed25519,
            ..Default::default()
        },
    )?;
    let server = signer.session_server().await?;
    let mut socket = browser(&server).await?;
    let context = context();
    let json = SignerPayloadJson::from_iter([("era".into(), json!("0x0200"))]);
    for (purpose, stage) in [
        (
            SignPurpose::ShieldInner,
            "scan 1 of 2 — the shielded transaction",
        ),
        (
            SignPurpose::ShieldCarrier,
            "scan 2 of 2 — the encrypted carrier",
        ),
    ] {
        let request = SigningRequest {
            call: &[1, 2],
            included_in_extrinsic: &[],
            included_in_signed_data: &[],
            payload: b"sign this",
            payload_json: &json,
            context: &context,
            purpose,
        };
        let phone = async {
            let message = read(&mut socket).await?;
            assert_eq!(message["summary"]["stage"], stage);
            assert_eq!(message["summary"]["deadlineSeconds"], 96.0);
            let signature = software.sign_raw(request.payload).await?;
            send(
                &mut socket,
                json!({"type":"signature","signature":hex::encode(signature)}),
            )
            .await?;
            Ok::<_, Box<dyn Error>>(())
        };
        let (result, phone) = tokio::join!(signer.sign_legacy(&request), phone);
        result?;
        phone?;
    }
    signer.close().await?;
    Ok(())
}
