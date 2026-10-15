//! Bounded public endpoint observations; no chain writes or quota escalation.

use std::{
    error::Error,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use futures::{SinkExt, StreamExt};
use rtao_rpc::{SessionConfig, policy, tls_config};
use serde_json::{Value, json};
use tokio::time::{Instant, sleep_until, timeout};
use tokio_tungstenite::{
    Connector, connect_async_tls_with_config,
    tungstenite::{Error as WsError, Message, protocol::WebSocketConfig},
};
use url::Url;

type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

const REQUESTS_PER_ENDPOINT: usize = 36;
const PERIOD: Duration = Duration::from_secs(1);
const DEADLINE: Duration = Duration::from_secs(15);
const SYSTEM_NUMBER: &str = "0x26aa394eea5630e07c48ae0c9558cef702a5c1b19ab7a04f536c519aca4983ac";
const ENDPOINTS: &[(&str, &str)] = &[
    (
        "finney-primary",
        "wss://entrypoint-finney.opentensor.ai:443",
    ),
    ("test-primary", "wss://test.finney.opentensor.ai:443"),
    ("devnet-primary", "wss://dev.chain.opentensor.ai:443"),
    ("finney-otf-lite", "wss://lite.chain.opentensor.ai:443"),
    ("finney-latent-lite", "wss://lite.sub.latent.to:443"),
    (
        "finney-otf-archive",
        "wss://archive.chain.opentensor.ai:443",
    ),
    ("finney-latent-archive", "wss://archive.sub.latent.to:443"),
];

fn timestamp() -> Result<f64> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs_f64())
}

fn emit(mut record: Value, endpoint: &str) -> Result<()> {
    record["endpoint"] = endpoint.into();
    record["unix_seconds"] = timestamp()?.into();
    println!("{}", serde_json::to_string(&record)?);
    Ok(())
}

fn response_headers(headers: &tokio_tungstenite::tungstenite::http::HeaderMap) -> Value {
    let mut output = serde_json::Map::new();
    for (name, value) in headers {
        let name = name.as_str();
        if name.starts_with("x-ratelimit-")
            || name.starts_with("ratelimit")
            || matches!(
                name,
                "retry-after" | "date" | "server" | "via" | "upgrade" | "connection"
            )
        {
            output.insert(
                name.into(),
                policy::safe_diagnostic(&String::from_utf8_lossy(value.as_bytes()), 500).into(),
            );
        }
    }
    Value::Object(output)
}

async fn observe(label: &str, endpoint: &str) -> Result<()> {
    let start = Instant::now();
    let config = SessionConfig::new(Url::parse(endpoint)?);
    let tls = tls_config(&config)?;
    let connection = connect_async_tls_with_config(
        endpoint,
        Some(
            WebSocketConfig::default()
                .max_message_size(Some(1_048_576))
                .max_frame_size(Some(1_048_576)),
        ),
        false,
        Some(Connector::Rustls(tls)),
    );
    let connected = match timeout(DEADLINE, connection).await {
        Ok(Ok(connected)) => connected,
        Ok(Err(WsError::Http(response))) => {
            let status = response.status().as_u16();
            emit(
                json!({
                    "event": "handshake", "label": label, "status": status,
                    "headers": response_headers(response.headers()),
                    "policy_refusal": policy::classify_handshake(status, response.headers(), response.body().as_deref()).map(|refusal| refusal.to_string()),
                    "body": response.body().as_deref().map(policy::response_detail),
                    "elapsed_ms": start.elapsed().as_secs_f64() * 1000.0,
                }),
                endpoint,
            )?;
            return Ok(());
        }
        Ok(Err(error)) => {
            emit(
                json!({"event": "connection_error", "label": label, "message": policy::safe_diagnostic(&error.to_string(), 500), "elapsed_ms": start.elapsed().as_secs_f64() * 1000.0}),
                endpoint,
            )?;
            return Ok(());
        }
        Err(error) => {
            emit(
                json!({"event": "connection_timeout", "label": label, "message": error.to_string(), "elapsed_ms": start.elapsed().as_secs_f64() * 1000.0}),
                endpoint,
            )?;
            return Ok(());
        }
    };
    let (mut socket, response) = connected;
    emit(
        json!({"event": "handshake", "label": label, "status": response.status().as_u16(), "headers": response_headers(response.headers()), "elapsed_ms": start.elapsed().as_secs_f64() * 1000.0}),
        endpoint,
    )?;
    let run_start = Instant::now();
    let mut last_sent = None;
    let mut successful = 0;
    let mut stopped = None;
    for index in 0..REQUESTS_PER_ENDPOINT {
        // Deadline uses the actual previous send. A delayed response can never
        // trigger a catch-up burst on the next iteration.
        if let Some(sent) = last_sent {
            sleep_until(sent + PERIOD).await;
        }
        let (method, params) = match index % 3 {
            0 => ("chain_getFinalizedHead", json!([])),
            1 => ("state_getStorage", json!([SYSTEM_NUMBER])),
            _ => ("system_health", json!([])),
        };
        let id = (index + 1) as u64;
        let frame = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let sent = Instant::now();
        let interval_ms =
            last_sent.map(|last: Instant| sent.duration_since(last).as_secs_f64() * 1000.0);
        last_sent = Some(sent);
        if let Err(error) = timeout(
            DEADLINE,
            socket.send(Message::Text(frame.to_string().into())),
        )
        .await?
        {
            emit(
                json!({"event": "send_error", "id": id, "method": method, "message": policy::safe_diagnostic(&error.to_string(), 500)}),
                endpoint,
            )?;
            stopped = Some("send_error");
            break;
        }
        let received = timeout(DEADLINE, async {
            loop {
                match socket.next().await {
                    Some(Ok(Message::Text(text))) => {
                        let value: Value = serde_json::from_str(text.as_str())?;
                        if value.get("id").and_then(Value::as_u64) == Some(id) { return Ok::<_, Box<dyn Error + Send + Sync>>(Some(value)); }
                    }
                    Some(Ok(Message::Binary(bytes))) => {
                        let value: Value = serde_json::from_slice(&bytes)?;
                        if value.get("id").and_then(Value::as_u64) == Some(id) { return Ok(Some(value)); }
                    }
                    Some(Ok(Message::Close(frame))) => {
                        emit(json!({"event": "server_close", "code": frame.as_ref().map(|frame| u16::from(frame.code)), "reason": frame.as_ref().map(|frame| policy::safe_diagnostic(&frame.reason, 500))}), endpoint)?;
                        return Ok(None);
                    }
                    Some(Ok(Message::Ping(_))) => { socket.flush().await?; }
                    Some(Ok(_)) => {}
                    Some(Err(error)) => return Err(error.into()),
                    None => return Ok(None),
                }
            }
        }).await;
        let response = match received {
            Ok(Ok(Some(value))) => value,
            Ok(Ok(None)) => {
                stopped = Some("server_close");
                break;
            }
            Ok(Err(error)) => {
                emit(
                    json!({"event": "response_error", "id": id, "method": method, "message": policy::safe_diagnostic(&error.to_string(), 500)}),
                    endpoint,
                )?;
                stopped = Some("response_error");
                break;
            }
            Err(error) => {
                emit(
                    json!({"event": "response_timeout", "id": id, "method": method, "message": error.to_string()}),
                    endpoint,
                )?;
                stopped = Some("response_timeout");
                break;
            }
        };
        let refusal = response.get("error").and_then(policy::classify_rpc_error);
        emit(
            json!({
                "event": "request", "id": id, "method": method,
                "send_offset_ms": sent.duration_since(run_start).as_secs_f64() * 1000.0,
                "send_interval_ms": interval_ms,
                "response_ms": sent.elapsed().as_secs_f64() * 1000.0,
                "response": response,
                "policy_refusal": refusal.as_ref().map(ToString::to_string),
            }),
            endpoint,
        )?;
        if refusal.is_some() {
            stopped = Some("policy_refusal");
            break;
        }
        if response.get("error").is_some() {
            stopped = Some("rpc_error");
            break;
        }
        successful += 1;
    }
    let close_result = timeout(Duration::from_secs(2), socket.close(None)).await;
    emit(
        json!({"event": "summary", "label": label, "successful": successful, "planned": REQUESTS_PER_ENDPOINT, "stopped": stopped, "elapsed_ms": run_start.elapsed().as_secs_f64() * 1000.0, "close_completed": matches!(close_result, Ok(Ok(()))), "configured_per_second": 1.0, "configured_burst": 1, "quota_ceiling_measured": false}),
        endpoint,
    )?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let chosen = std::env::args().skip(1).collect::<Vec<_>>();
    if !chosen.is_empty()
        && chosen
            .iter()
            .any(|label| !ENDPOINTS.iter().any(|(known, _)| label == known))
    {
        return Err("arguments must be endpoint labels from ENDPOINTS".into());
    }
    emit(
        json!({"event": "run", "planned_requests_per_endpoint": REQUESTS_PER_ENDPOINT, "period_seconds": 1, "deadline_seconds": 15, "concurrency": 1, "chain_writes": false}),
        "",
    )?;
    for (label, endpoint) in ENDPOINTS {
        if (chosen.is_empty() || chosen.iter().any(|chosen| chosen == label))
            && let Err(error) = observe(label, endpoint).await
        {
            emit(
                json!({"event": "observation_error", "label": label, "message": policy::safe_diagnostic(&error.to_string(), 500)}),
                endpoint,
            )?;
        }
    }
    Ok(())
}
