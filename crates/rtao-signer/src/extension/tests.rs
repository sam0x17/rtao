// Derived from RaoFoundation/subtensor sdk/python/tests/unit/test_extension_bridge.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
#![allow(clippy::indexing_slicing)]

use std::{error::Error, sync::Arc, time::Duration};

use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::timeout,
};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async,
    tungstenite::{Message, client::IntoClientRequest, protocol::frame::coding::CloseCode},
};

use super::*;
use crate::{CryptoType, Signer};

type TestResult = Result<(), Box<dyn Error>>;
type BrowserSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;

async fn server() -> Result<BridgeServer, BridgeError> {
    let mut server = BridgeServer::new(
        "127.0.0.1:0"
            .parse()
            .map_err(|_| error("bad fixture address"))?,
    )?;
    server.start().await?;
    Ok(server)
}
fn client(server: &BridgeServer) -> Result<BridgeClient, BridgeError> {
    BridgeClient::new(
        &server.ws_url(),
        server
            .client_token()
            .ok_or_else(|| error("missing fixture token"))?,
    )
}
async fn browser(server: &BridgeServer) -> Result<BrowserSocket, Box<dyn Error>> {
    let (mut socket, _) = connect_async(server.ws_url()).await?;
    socket
        .send(Message::Text(
            json!({"role":"bridge","session":server.session_id()})
                .to_string()
                .into(),
        ))
        .await?;
    server.wait_until_ready(Duration::from_secs(2)).await?;
    Ok(socket)
}
async fn read(socket: &mut BrowserSocket) -> Result<Value, Box<dyn Error>> {
    Ok(timeout(Duration::from_secs(2), client::receive_json(socket)).await??)
}
async fn answer(
    socket: &mut BrowserSocket,
    request: &Value,
    result: Value,
) -> Result<(), Box<dyn Error>> {
    socket
        .send(Message::Text(
            json!({"id":request.get("id"),"result":result})
                .to_string()
                .into(),
        ))
        .await?;
    Ok(())
}
fn account(name: &str, source: &str, byte: u8) -> ExtensionAccount {
    ExtensionAccount {
        address: crate::AccountId([byte; 32]).to_ss58(42),
        name: name.into(),
        source: source.into(),
        account_type: "sr25519".into(),
    }
}

// pyt: tests/unit/test_extension_bridge.py::test_bridge_rejects_client_without_token
#[tokio::test]
async fn bridge_rejects_client_without_token() -> TestResult {
    let mut server = server().await?;
    let client = BridgeClient::new(&server.ws_url(), "")?;
    assert!(
        client
            .connect()
            .await
            .is_err_and(|e| e.to_string().contains("token is missing"))
    );
    server.stop().await?;
    Ok(())
}
// pyt: tests/unit/test_extension_bridge.py::test_bridge_rejects_client_with_wrong_token
#[tokio::test]
async fn bridge_rejects_client_with_wrong_token() -> TestResult {
    let mut server = server().await?;
    let client = BridgeClient::new(&server.ws_url(), "not-the-server-token")?;
    assert!(
        client
            .connect()
            .await
            .is_err_and(|e| e.to_string().contains("rejected the client token"))
    );
    server.stop().await?;
    Ok(())
}
// pyt: tests/unit/test_extension_bridge.py::test_bridge_accepts_client_with_matching_token
#[tokio::test]
async fn bridge_accepts_client_with_matching_token() -> TestResult {
    let mut server = server().await?;
    let client = client(&server)?;
    client.connect().await?;
    let status = client.request("bridge.status", json!({})).await?;
    assert_eq!(
        status,
        json!({"browser_connected":false,"account_count":0,"http_url":server.http_url(),"session_id":server.session_id()})
    );
    client.close().await;
    server.stop().await?;
    Ok(())
}
// pyt: tests/unit/test_extension_bridge.py::test_bridge_browser_requires_matching_session
#[tokio::test]
async fn bridge_browser_requires_matching_session() -> TestResult {
    let mut server = server().await?;
    let (mut socket, _) = connect_async(server.ws_url()).await?;
    socket
        .send(Message::Text(
            json!({"role":"bridge","session":"stale-session-id"})
                .to_string()
                .into(),
        ))
        .await?;
    let message = timeout(Duration::from_secs(2), socket.next())
        .await?
        .ok_or("missing close")??;
    assert!(matches!(message,Message::Close(Some(frame)) if frame.code==CloseCode::Policy));
    server.stop().await?;
    Ok(())
}
// pyt: tests/unit/test_extension_bridge.py::test_client_reports_transaction_result
#[tokio::test]
async fn client_reports_transaction_result() -> TestResult {
    let mut server = server().await?;
    let mut browser = browser(&server).await?;
    let client = Arc::new(client(&server)?);
    for success in [false, true] {
        let c = client.clone();
        let task = tokio::spawn(async move { c.report_transaction_result(success).await });
        let request = read(&mut browser).await?;
        assert_eq!(request["method"], "transaction.result");
        assert_eq!(request["params"], json!({"success":success}));
        answer(&mut browser, &request, json!({"acknowledged":true})).await?;
        task.await??;
    }
    client.close().await;
    server.stop().await?;
    Ok(())
}
#[tokio::test]
async fn extrinsic_stage_omits_empty_labels_and_preserves_nonempty_labels() -> TestResult {
    let mut server = server().await?;
    let mut browser = browser(&server).await?;
    let client = Arc::new(client(&server)?);
    for (stage, more_coming) in [
        (None, false),
        (Some(""), true),
        (Some("approve 1 of 2"), true),
    ] {
        let c = client.clone();
        let task = tokio::spawn(async move {
            c.sign_extrinsic_payload(json!({"method":"0x01"}), stage, more_coming)
                .await
        });
        let request = read(&mut browser).await?;
        let expected = match stage {
            Some("approve 1 of 2") => {
                json!({"payload":{"method":"0x01"},"moreComing":true,"stage":"approve 1 of 2"})
            }
            _ => json!({"payload":{"method":"0x01"},"moreComing":more_coming}),
        };
        assert_eq!(request["method"], "extrinsic.sign");
        assert_eq!(request["params"], expected);
        answer(&mut browser, &request, json!({"signature":"0x00"})).await?;
        assert_eq!(task.await??, json!({"signature":"0x00"}));
    }
    client.close().await;
    server.stop().await?;
    Ok(())
}
#[tokio::test]
async fn bridge_serves_exact_pinned_page_without_token() -> TestResult {
    let mut server = server().await?;
    let mut socket = TcpStream::connect(server.address()).await?;
    socket
        .write_all(
            format!(
                "GET /?session=example HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
                server.address()
            )
            .as_bytes(),
        )
        .await?;
    let mut response = Vec::new();
    timeout(Duration::from_secs(2), socket.read_to_end(&mut response)).await??;
    let marker = response
        .windows(4)
        .position(|v| v == b"\r\n\r\n")
        .ok_or("missing HTTP headers")?
        + 4;
    assert_eq!(
        &response[marker..],
        include_bytes!("../../../../vendor/upstream/extension/static/bridge.html")
    );
    assert!(
        !String::from_utf8_lossy(&response).contains(server.client_token().ok_or("missing token")?)
    );
    server.stop().await?;
    Ok(())
}
#[tokio::test]
async fn bridge_rejects_cross_origin_upgrade_and_rebound_host() -> TestResult {
    let mut server = server().await?;
    let mut request = server.ws_url().into_client_request()?;
    request
        .headers_mut()
        .insert("origin", "https://attacker.invalid".parse()?);
    assert!(connect_async(request).await.is_err());
    let mut request = server.ws_url().into_client_request()?;
    request
        .headers_mut()
        .insert("host", "attacker.invalid".parse()?);
    assert!(connect_async(request).await.is_err());
    server.stop().await?;
    Ok(())
}
#[tokio::test]
async fn bridge_routes_payload_and_browser_error() -> TestResult {
    let mut server = server().await?;
    let mut browser = browser(&server).await?;
    let client = Arc::new(client(&server)?);
    let c = client.clone();
    let task = tokio::spawn(async move { c.request("accounts.list", json!({})).await });
    let request = read(&mut browser).await?;
    assert_eq!(request["method"], "accounts.list");
    assert_eq!(request["params"], json!({}));
    answer(&mut browser, &request, json!({"accounts":[]})).await?;
    assert_eq!(task.await??, json!({"accounts":[]}));
    let c = client.clone();
    let task = tokio::spawn(async move {
        c.request("bytes.sign", json!({"address":"x","data":"0x01"}))
            .await
    });
    let request = read(&mut browser).await?;
    browser
        .send(Message::Text(
            json!({"id":request["id"],"error":{"message":"user rejected request"}})
                .to_string()
                .into(),
        ))
        .await?;
    assert_eq!(
        task.await?.err().ok_or("expected rejection")?.to_string(),
        "user rejected request"
    );
    client.close().await;
    server.stop().await?;
    Ok(())
}
#[tokio::test]
async fn replacing_browser_cancels_old_work_but_keeps_new_tab() -> TestResult {
    let mut server = server().await?;
    let mut old = browser(&server).await?;
    let client = Arc::new(client(&server)?);
    let c = client.clone();
    let pending = tokio::spawn(async move { c.request("accounts.list", json!({})).await });
    let _request = read(&mut old).await?;
    let mut new = browser(&server).await?;
    let closed = timeout(Duration::from_secs(2), old.next())
        .await?
        .ok_or("missing replacement close")??;
    assert!(
        matches!(closed,Message::Close(Some(frame)) if frame.code==CloseCode::Restart && frame.reason=="replaced by new bridge tab")
    );
    assert!(timeout(Duration::from_secs(2), pending).await??.is_err());
    let c = client.clone();
    let request = tokio::spawn(async move { c.request("accounts.list", json!({})).await });
    let forwarded = read(&mut new).await?;
    answer(&mut new, &forwarded, json!({"accounts":[]})).await?;
    assert_eq!(request.await??, json!({"accounts":[]}));
    client.close().await;
    server.stop().await?;
    Ok(())
}
#[tokio::test]
async fn shutdown_cancels_pending_browser_requests() -> TestResult {
    let mut server = server().await?;
    let mut browser = browser(&server).await?;
    let client = Arc::new(client(&server)?);
    let c = client.clone();
    let pending = tokio::spawn(async move { c.request("accounts.list", json!({})).await });
    let _ = read(&mut browser).await?;
    server.stop().await?;
    assert!(timeout(Duration::from_secs(2), pending).await??.is_err());
    Ok(())
}
#[test]
fn connection_targets_never_send_tokens_off_host() -> TestResult {
    for url in [
        "ws://example.com/ws",
        "ws://localhost/ws",
        "ws://127.0.0.1.example.com/ws",
        "ws://192.0.2.1/ws",
        "http://127.0.0.1/ws",
        "ws://user:password@127.0.0.1/ws",
        "ws://127.0.0.1/ws?token=x",
        "ws://127.0.0.1/ws#x",
    ] {
        assert!(BridgeClient::new(url, "token").is_err(), "{url}");
    }
    assert!(BridgeClient::new("ws://127.0.0.1:39295/ws", "token").is_ok());
    assert!(BridgeClient::new("wss://[::1]:39295/ws", "token").is_ok());
    assert_eq!(
        client::session_url("http://127.0.0.1/")?.as_str(),
        "ws://127.0.0.1:39295/ws"
    );
    assert!(BridgeServer::new("0.0.0.0:39295".parse()?).is_err());
    Ok(())
}
#[test]
fn account_metadata_fallbacks_match_python() -> TestResult {
    let accounts = client::accounts_from_result(&json!({"accounts":[null,{}, {"address":17},
        {"address":"a"},{"address":"b","meta":{"name":"Alice","source":"talisman"},"name":"ignored","type":"ed25519"},
        {"address":"c","meta":{"name":"","source":null},"name":"Carol","source":"polkadot-js"}]}))?;
    assert_eq!(
        accounts,
        vec![
            ExtensionAccount {
                address: "a".into(),
                name: "a".into(),
                source: "unknown".into(),
                account_type: "sr25519".into()
            },
            ExtensionAccount {
                address: "b".into(),
                name: "Alice".into(),
                source: "talisman".into(),
                account_type: "ed25519".into()
            },
            ExtensionAccount {
                address: "c".into(),
                name: "Carol".into(),
                source: "polkadot-js".into(),
                account_type: "sr25519".into()
            },
        ]
    );
    Ok(())
}
// pyt: tests/unit/test_extension_signer.py::test_account_crypto_type_sr25519
#[test]
fn account_crypto_type_sr25519() -> TestResult {
    assert_eq!(account_crypto_type(" SR25519 ")?, CryptoType::Sr25519);
    Ok(())
}
// pyt: tests/unit/test_extension_signer.py::test_account_crypto_type_ed25519
#[test]
fn account_crypto_type_ed25519() -> TestResult {
    assert_eq!(account_crypto_type("ed25519")?, CryptoType::Ed25519);
    Ok(())
}
#[test]
fn account_crypto_type_matches_python_edge_whitespace_without_removing_interior_bytes() -> TestResult
{
    for separator in ['\u{1c}', '\u{1d}', '\u{1e}', '\u{1f}'] {
        for (algorithm, expected) in [
            ("SR25519", CryptoType::Sr25519),
            ("ED25519", CryptoType::Ed25519),
        ] {
            assert_eq!(
                account_crypto_type(&format!(" {separator}{algorithm}{separator}\u{3000}"))?,
                expected
            );
        }
        assert!(account_crypto_type(&format!("sr{separator}25519")).is_err());
    }
    Ok(())
}
// pyt: tests/unit/test_extension_signer.py::test_account_crypto_type_rejects_unsupported
#[test]
fn account_crypto_type_rejects_unsupported() {
    for algorithm in ["ecdsa", "ethereum", "ECDSA"] {
        assert!(account_crypto_type(algorithm).is_err_and(|e| {
            e.to_string()
                .contains("not supported for Substrate signing")
        }));
    }
    assert!(account_crypto_type("unknown").is_err());
}
#[test]
fn selection_errors_preserve_filtered_hints_and_explicit_address() -> TestResult {
    let accounts = [
        account("Alice", "chosen", 1),
        account("Bob", "other", 2),
        account("Carol", "chosen", 3),
    ];
    let mut options = AccountSelectionOptions {
        source: Some("chosen".into()),
        ..Default::default()
    };
    let ambiguous = signer::select_from_accounts(&accounts, &options)
        .err()
        .ok_or("expected ambiguity")?;
    assert_eq!(
        ambiguous.to_string(),
        format!(
            "multiple extension accounts available; pass --signer-address or run in a terminal: Alice ({}, chosen), Carol ({}, chosen)",
            accounts[0].address, accounts[2].address
        )
    );
    options.source = Some("missing".into());
    let missing = signer::select_from_accounts(&accounts, &options)
        .err()
        .ok_or("expected no matching filter")?;
    assert_eq!(
        missing.to_string(),
        format!(
            "no extension account matched the given filters; available: Alice ({}, chosen), Bob ({}, other), Carol ({}, chosen)",
            accounts[0].address, accounts[1].address, accounts[2].address
        )
    );
    options.address = Some("missing".into());
    assert_eq!(
        signer::select_from_accounts(&accounts, &options)
            .err()
            .ok_or("expected missing explicit address")?
            .to_string(),
        "extension account 'missing' not found"
    );
    Ok(())
}
#[test]
fn account_selection_preserves_address_precedence_and_saved_uniqueness() -> TestResult {
    let alice = account("Alice", "talisman", 1);
    let bob = account("Bob", "polkadot-js", 2);
    let accounts = vec![alice.clone(), bob.clone()];
    assert!(signer::select_from_accounts(&accounts, &AccountSelectionOptions::default()).is_err());
    let options = AccountSelectionOptions {
        address: Some(alice.address.clone()),
        source: Some("unmatched".into()),
        name: Some("unmatched".into()),
        default_address: None,
    };
    assert_eq!(
        signer::select_from_accounts(&accounts, &options)?.account,
        alice
    );
    let options = AccountSelectionOptions {
        default_address: Some(bob.address.clone()),
        ..Default::default()
    };
    assert_eq!(
        signer::select_from_accounts(&accounts, &options)?.account,
        bob
    );
    let options = AccountSelectionOptions {
        source: Some("talisman".into()),
        ..Default::default()
    };
    assert_eq!(
        signer::select_from_accounts(&accounts, &options)?.public_key,
        [1; 32]
    );
    Ok(())
}
// pyt: tests/unit/test_extension_signer.py::test_two_stage_labels_shielded_payloads
#[tokio::test]
async fn two_stage_labels_shielded_payloads() -> TestResult {
    let mut server = server().await?;
    let mut browser = browser(&server).await?;
    let client = Arc::new(client(&server)?);
    let selected = signer::select_from_accounts(
        &[account("Alice", "talisman", 1)],
        &AccountSelectionOptions::default(),
    )?;
    let address = selected.account.address.clone();
    let signer = Arc::new(ExtensionSigner::new(selected, client)?);
    signer.set_two_stage(true).await;
    for (stage, more) in [
        ("approve 1 of 2 — the shielded transaction", true),
        ("approve 2 of 2 — the encrypted carrier", false),
    ] {
        let signer = signer.clone();
        let task = tokio::spawn(async move {
            signer
                .sign_extrinsic_payload(
                    &serde_json::from_value(json!({"method":"0x01","address":"wrong"}))
                        .map_err(|_| error("bad fixture"))?,
                )
                .await
        });
        let request = read(&mut browser).await?;
        assert_eq!(request["method"], "extrinsic.sign");
        assert_eq!(request["params"]["stage"], stage);
        assert_eq!(request["params"]["moreComing"], more);
        assert_eq!(request["params"]["payload"]["address"], address);
        answer(&mut browser, &request, json!({"signature":"0x00"})).await?;
        assert_eq!(task.await??, json!({"signature":"0x00"}));
    }
    signer.close().await;
    server.stop().await?;
    Ok(())
}
#[tokio::test]
async fn extension_raw_signing_preserves_exact_bytes_and_algorithm() -> TestResult {
    let mut server = server().await?;
    let mut browser = browser(&server).await?;
    let selected = signer::select_from_accounts(
        &[account("Alice", "talisman", 1)],
        &AccountSelectionOptions::default(),
    )?;
    let signer = Arc::new(ExtensionSigner::new(selected, Arc::new(client(&server)?))?);
    let s = signer.clone();
    let task = tokio::spawn(async move { s.sign_raw(&[0, 255, 1]).await });
    let request = read(&mut browser).await?;
    assert_eq!(request["method"], "bytes.sign");
    assert_eq!(request["params"]["data"], "0x00ff01");
    assert_eq!(request["params"]["type"], "bytes");
    answer(
        &mut browser,
        &request,
        json!({"signature":format!("0x01{}",hex::encode([7;64]))}),
    )
    .await?;
    assert_eq!(task.await??, [7; 64]);
    signer.close().await;
    server.stop().await?;
    Ok(())
}

#[test]
fn payload_bounds_refuse_deep_or_oversized_caller_json() {
    assert!(super::bounded_value(&json!("x".repeat(super::MESSAGE_LIMIT + 1))).is_err());
    let mut value = Value::Null;
    for _ in 0..66 {
        value = json!([value]);
    }
    assert!(super::bounded_value(&value).is_err());
}
#[test]
fn signatures_are_bounded_and_never_change_negotiated_crypto() -> TestResult {
    let raw = json!({"signature":hex::encode([7;64])});
    assert_eq!(
        signer::decode_signature(&raw, CryptoType::Sr25519)?.bytes,
        [7; 64]
    );
    let prefixed = json!({"signature":format!("0x01{}",hex::encode([7;64]))});
    assert!(signer::decode_signature(&prefixed, CryptoType::Ed25519).is_err());
    for bad in [
        json!({}),
        json!({"signature":"0x00"}),
        json!({"signature":"gg".repeat(64)}),
        json!({"signature":"00".repeat(1024)}),
    ] {
        assert!(signer::decode_signature(&bad, CryptoType::Sr25519).is_err());
    }
    Ok(())
}
#[test]
fn library_defaults_require_explicit_process_launch_permissions() -> TestResult {
    assert!(!BridgeOptions::default().open_browser);
    let options = EnsureBridgeOptions::from_environment()?;
    assert!(!options.allow_spawn);
    assert!(!options.open_browser);
    assert!(!options.fresh);
    assert_eq!(
        bridge_page_url("127.0.0.1:39295".parse()?, "a&b"),
        "http://127.0.0.1:39295/?session=a%26b"
    );
    Ok(())
}

#[test]
fn malformed_metadata_matches_python_truthiness_and_rejects_bad_crypto() -> TestResult {
    let accounts = client::accounts_from_result(&json!({"accounts":[
        {"address":"a","meta":{"name":0,"source":[]},"name":"fallback","source":"valid","type":true},
        {"address":"b","name":[true,"x"],"source":{"key":false},"type":17},
        {"address":"c","name":{},"source":0,"type":[]}
    ]}))?;
    assert_eq!(accounts[0].name, "fallback");
    assert_eq!(accounts[0].source, "valid");
    assert_eq!(accounts[0].account_type, "True");
    assert_eq!(accounts[1].name, "[True, 'x']");
    assert_eq!(accounts[1].source, "{'key': False}");
    assert_eq!(accounts[1].account_type, "17");
    assert_eq!(accounts[2].name, "c");
    assert_eq!(accounts[2].source, "unknown");
    assert_eq!(accounts[2].account_type, "sr25519");
    assert!(account_crypto_type(&accounts[0].account_type).is_err());
    assert!(account_crypto_type(&accounts[1].account_type).is_err());
    let raw = format!(
        "{{\"accounts\":[{{\"address\":\"large\",\"name\":{},\"source\":1e400,\"type\":{}}}]}}",
        "9".repeat(400),
        "7".repeat(400)
    );
    let large = client::accounts_from_result(&serde_json::from_str(&raw)?)?;
    assert_eq!(large[0].name, "9".repeat(400));
    assert_eq!(large[0].source, "inf");
    assert_eq!(large[0].account_type, "7".repeat(400));
    assert!(account_crypto_type(&large[0].account_type).is_err());
    Ok(())
}

#[tokio::test]
async fn close_cancels_active_and_queued_approvals() -> TestResult {
    let mut server = server().await?;
    let mut browser = browser(&server).await?;
    let client = Arc::new(client(&server)?);
    let c = client.clone();
    let active = tokio::spawn(async move { c.request("accounts.list", json!({})).await });
    let _ = read(&mut browser).await?;
    let c = client.clone();
    let (started, waiting) = tokio::sync::oneshot::channel();
    let queued = tokio::spawn(async move {
        let _ = started.send(());
        c.request("accounts.list", json!({})).await
    });
    waiting.await?;
    timeout(Duration::from_secs(2), client.close()).await?;
    assert!(timeout(Duration::from_secs(2), active).await??.is_err());
    assert!(timeout(Duration::from_secs(2), queued).await??.is_err());
    server.stop().await?;
    Ok(())
}

#[tokio::test]
async fn request_deadline_includes_waiting_behind_an_approval() -> TestResult {
    let mut server = server().await?;
    let mut browser = browser(&server).await?;
    let client = Arc::new(client(&server)?);
    let c = client.clone();
    let active = tokio::spawn(async move { c.request("accounts.list", json!({})).await });
    let request = read(&mut browser).await?;
    let timed = timeout(
        Duration::from_secs(2),
        client.request_with_timeout("accounts.list", json!({}), Duration::from_millis(20)),
    )
    .await?;
    assert!(timed.is_err_and(|e| e.to_string().contains("timed out")));
    answer(&mut browser, &request, json!({"accounts":[]})).await?;
    assert!(active.await?.is_ok());
    client.close().await;
    server.stop().await?;
    Ok(())
}

#[tokio::test]
async fn signer_close_cancels_queued_shield_stages_without_reconnecting() -> TestResult {
    let mut server = server().await?;
    let mut browser = browser(&server).await?;
    let selected = signer::select_from_accounts(
        &[account("Alice", "talisman", 1)],
        &AccountSelectionOptions::default(),
    )?;
    let signer = Arc::new(ExtensionSigner::new(selected, Arc::new(client(&server)?))?);
    let first = signer.clone();
    let active =
        tokio::spawn(async move { first.sign_extrinsic_payload(&serde_json::Map::new()).await });
    let _ = read(&mut browser).await?;
    let second = signer.clone();
    let (started, waiting) = tokio::sync::oneshot::channel();
    let queued = tokio::spawn(async move {
        let _ = started.send(());
        second.sign_extrinsic_payload(&serde_json::Map::new()).await
    });
    waiting.await?;
    timeout(Duration::from_secs(2), signer.close()).await?;
    assert!(timeout(Duration::from_secs(2), active).await??.is_err());
    assert!(timeout(Duration::from_secs(2), queued).await??.is_err());
    assert!(
        timeout(Duration::from_millis(25), browser.next())
            .await
            .is_err()
    );
    server.stop().await?;
    Ok(())
}

#[tokio::test]
async fn shutdown_cancels_upgraded_connections_before_their_hello() -> TestResult {
    let mut server = server().await?;
    let (mut websocket, _) = connect_async(server.ws_url()).await?;
    let probe = server.task_probe()?;
    server.stop().await?;
    timeout(Duration::from_secs(2), async {
        while !probe.all_finished() {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    let reply = timeout(Duration::from_secs(2), websocket.next()).await?;
    assert!(!matches!(reply, Some(Ok(Message::Text(_)))));
    Ok(())
}

#[tokio::test]
async fn expired_startup_refuses_spawn_without_touching_state() -> TestResult {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    drop(listener);
    let root =
        std::env::temp_dir().join(format!("rtao-extension-uncreated-{}", super::random_id()?));
    let options = EnsureBridgeOptions {
        url: bridge_ws_url(address),
        paths: BridgePaths {
            token_file: root.join("token"),
            pid_file: root.join("pid"),
        },
        allow_spawn: true,
        open_browser: false,
        browser: None,
        fresh: false,
        startup_timeout: Duration::ZERO,
        extension_timeout: Duration::ZERO,
        on_waiting: None,
    };
    assert!(
        ensure_bridge(&options)
            .await
            .is_err_and(|e| e.to_string().contains("deadline"))
    );
    assert!(!root.exists());
    Ok(())
}

#[cfg(unix)]
#[test]
fn private_files_refuse_symlinks_hardlinks_oversize_and_unsafe_modes() -> TestResult {
    session::private_file_regressions()
}

#[cfg(unix)]
#[tokio::test]
async fn publication_rejects_path_aliases_and_reserves_live_state() -> TestResult {
    let mut server = server().await?;
    session::publication_regressions(&server)?;
    server.stop().await?;
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn fresh_ensure_reuses_authenticated_foreground_without_signalling() -> TestResult {
    let mut server = server().await?;
    let mut browser = browser(&server).await?;
    let (paths, _cleanup) = session::foreground_fixture_paths()?;
    let published = paths.publish(&server)?;
    let url = server.ws_url();
    let expected = url.clone();
    let task = tokio::spawn(async move {
        ensure_bridge(&EnsureBridgeOptions {
            url,
            paths,
            allow_spawn: true,
            open_browser: false,
            browser: None,
            fresh: true,
            startup_timeout: Duration::from_secs(5),
            extension_timeout: Duration::from_secs(5),
            on_waiting: None,
        })
        .await
    });
    let request = read(&mut browser).await?;
    assert_eq!(request["method"], "accounts.list");
    answer(
        &mut browser,
        &request,
        json!({"accounts":[{"address":"public-account"}]}),
    )
    .await?;
    assert_eq!(task.await??, expected);
    drop(published);
    server.stop().await?;
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn ensure_reports_the_manual_session_page_and_keeps_it_in_timeout() -> TestResult {
    let mut server = server().await?;
    let (paths, _cleanup) = session::foreground_fixture_paths()?;
    let published = paths.publish(&server)?;
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let events = seen.clone();
    let options = EnsureBridgeOptions {
        url: server.ws_url(),
        paths,
        allow_spawn: false,
        open_browser: false,
        browser: None,
        fresh: false,
        startup_timeout: Duration::from_secs(2),
        extension_timeout: Duration::from_millis(20),
        on_waiting: Some(Arc::new(move |page, status| {
            if let Ok(mut events) = events.lock() {
                events.push((page.to_owned(), status.clone()));
            }
        })),
    };
    let failure = ensure_bridge(&options)
        .await
        .err()
        .ok_or("expected readiness timeout")?;
    let page = server.page_url().ok_or("session page")?;
    assert_eq!(
        failure.to_string(),
        format!(
            "extension not ready; open the session page and authorize your wallet extension: {page}"
        )
    );
    assert_eq!(
        *seen.lock().map_err(|_| "observer lock")?,
        [(
            page,
            json!({"browser_connected":false,"account_count":0,"http_url":server.http_url(),"session_id":server.session_id()})
        )]
    );
    assert!(format!("{options:?}").contains("on_waiting: true"));
    drop(published);
    server.stop().await?;
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn ensure_observer_sees_pending_status_then_authorization_completes() -> TestResult {
    let mut server = server().await?;
    let mut browser = browser(&server).await?;
    let (paths, _cleanup) = session::foreground_fixture_paths()?;
    let published = paths.publish(&server)?;
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let events = seen.clone();
    let options = EnsureBridgeOptions {
        url: server.ws_url(),
        paths,
        allow_spawn: false,
        open_browser: false,
        browser: None,
        fresh: false,
        startup_timeout: Duration::from_secs(2),
        extension_timeout: Duration::from_secs(2),
        on_waiting: Some(Arc::new(move |page, status| {
            if let Ok(mut events) = events.lock() {
                events.push((page.to_owned(), status.clone()));
            }
        })),
    };
    let expected = options.url.clone();
    let task = tokio::spawn(async move { ensure_bridge(&options).await });
    let first = read(&mut browser).await?;
    assert_eq!(first["method"], "accounts.list");
    answer(&mut browser, &first, json!({"accounts":[]})).await?;
    let next = read(&mut browser).await?;
    assert_eq!(next["method"], "accounts.list");
    answer(
        &mut browser,
        &next,
        json!({"accounts":[{"address":"public-account"}]}),
    )
    .await?;
    assert_eq!(task.await??, expected);
    assert_eq!(
        *seen.lock().map_err(|_| "observer lock")?,
        [(
            server.page_url().ok_or("session page")?,
            json!({"browser_connected":true,"account_count":0,"http_url":server.http_url(),"session_id":server.session_id()})
        )]
    );
    drop(published);
    server.stop().await?;
    Ok(())
}
