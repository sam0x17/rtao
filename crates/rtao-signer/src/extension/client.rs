// Derived from RaoFoundation/subtensor sdk/python/bittensor/extension/client.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
use std::{
    net::{IpAddr, SocketAddr},
    sync::Arc,
};

use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::{
    net::TcpStream,
    sync::{Mutex, watch},
    time::timeout,
};
use tokio_tungstenite::{
    Connector, MaybeTlsStream, WebSocketStream, connect_async_tls_with_config,
    tungstenite::{Message, protocol::WebSocketConfig},
};
use url::Url;
use zeroize::Zeroizing;

use super::{
    BridgeError, CONNECT_TIMEOUT, MESSAGE_LIMIT, REQUEST_TIMEOUT, SEND_TIMEOUT, TEXT_LIMIT,
    bounded_json, error, random_id, wire_error,
};

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Public account advertised by a browser extension.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[doc(alias = "bittensor.extension.ExtensionAccount")]
pub struct ExtensionAccount {
    /// Account SS58 spelling supplied by the extension.
    pub address: String,
    /// Human-readable account name.
    pub name: String,
    /// Extension source, such as `polkadot-js` or `talisman`.
    pub source: String,
    /// Extension algorithm name (`sr25519` or `ed25519`).
    #[serde(rename = "type")]
    pub account_type: String,
}

/// Serialized RPC client for an authenticated loopback bridge.
///
/// No socket or file is opened by [`Self::new`]. Each connection uses one token;
/// failed or cancelled requests discard the socket to avoid reusing stale replies.
#[doc(alias = "bittensor.extension.BridgeClient")]
pub struct BridgeClient {
    pub(crate) url: Url,
    token: Zeroizing<String>,
    socket: Mutex<Option<Socket>>,
    cancel: watch::Sender<u64>,
}

impl std::fmt::Debug for BridgeClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BridgeClient")
            .field("url", &self.url.as_str())
            .finish_non_exhaustive()
    }
}

impl BridgeClient {
    /// Construct a client for a numeric loopback URL and an explicit token.
    ///
    /// Hostnames, credentials, fragments and non-WebSocket URLs are refused;
    /// resolving `localhost` through DNS could send the local token off-host.
    pub fn new(url: &str, token: impl Into<String>) -> Result<Self, BridgeError> {
        let url = validate_url(url)?;
        let token = Zeroizing::new(token.into());
        if token.len() > TEXT_LIMIT || token.chars().any(char::is_control) {
            return Err(error("invalid extension bridge client token"));
        }
        Ok(Self {
            url,
            token,
            socket: Mutex::new(None),
            cancel: watch::channel(0).0,
        })
    }

    /// Connect and authenticate, or reuse the authenticated connection.
    pub async fn connect(&self) -> Result<(), BridgeError> {
        let mut cancelled = self.cancel.subscribe();
        tokio::select! {
            result=timeout(CONNECT_TIMEOUT,async{
                let mut socket = self.socket.lock().await;
                if socket.is_none() {*socket = Some(self.open().await?);}
                Ok(())
            })=>result.map_err(|_|error("extension bridge connection timed out"))?,
            _=cancelled.changed()=>Err(error("extension bridge connection closed")),
        }
    }

    async fn open(&self) -> Result<Socket, BridgeError> {
        if self.token.is_empty() {
            return Err(error(
                "extension bridge client token is missing; explicitly start the bridge first",
            ));
        }
        let connector = if self.url.scheme() == "wss" {
            Some(Connector::Rustls(tls_config()?))
        } else {
            None
        };
        let config = WebSocketConfig::default()
            .max_message_size(Some(MESSAGE_LIMIT))
            .max_frame_size(Some(MESSAGE_LIMIT));
        let (mut socket, _) = timeout(
            CONNECT_TIMEOUT,
            connect_async_tls_with_config(self.url.as_str(), Some(config), false, connector),
        )
        .await
        .map_err(|_| error("extension bridge connection timed out"))?
        .map_err(|_| error("cannot connect to the extension bridge"))?;
        // Tungstenite owns frame storage; it cannot promise wiping every transport
        // copy. Keep our serialized owner guarded before transferring that copy.
        #[derive(Serialize)]
        struct Hello<'a> {
            role: &'static str,
            token: &'a str,
        }
        let hello = Zeroizing::new(
            serde_json::to_string(&Hello {
                role: "client",
                token: self.token.as_str(),
            })
            .map_err(|_| error("cannot encode bridge authentication"))?,
        );
        send_text(&mut socket, hello.as_str().to_owned()).await?;
        let hello = timeout(CONNECT_TIMEOUT, receive_json(&mut socket))
            .await
            .map_err(|_| error("extension bridge rejected the client token"))?
            .map_err(|_| error("extension bridge rejected the client token"))?;
        if hello.get("role").and_then(Value::as_str) != Some("ok") {
            return Err(error("extension bridge rejected the client token"));
        }
        Ok(socket)
    }

    /// Close the connection. A subsequent explicit request may reconnect.
    pub async fn close(&self) {
        self.cancel
            .send_modify(|generation| *generation = generation.wrapping_add(1));
        let _ = timeout(SEND_TIMEOUT, async {
            if let Some(mut socket) = self.socket.lock().await.take() {
                let _ = timeout(SEND_TIMEOUT, socket.close(None)).await;
            }
        })
        .await;
    }

    /// Send one protocol request, waiting at most five minutes for approval.
    pub async fn request(&self, method: &str, params: Value) -> Result<Value, BridgeError> {
        self.request_with_timeout(method, params, REQUEST_TIMEOUT)
            .await
    }

    pub(super) async fn request_with_timeout(
        &self,
        method: &str,
        params: Value,
        wait: std::time::Duration,
    ) -> Result<Value, BridgeError> {
        if method.len() > 128 || !params.is_object() {
            return Err(error("invalid bridge request"));
        }
        super::bounded_value(&params)?;
        let id = random_id()?;
        let encoded = json!({"id": id, "method":method, "params":params}).to_string();
        if encoded.len() > MESSAGE_LIMIT {
            return Err(error("bridge message exceeds the size limit"));
        }
        let mut cancelled = self.cancel.subscribe();
        tokio::select! {
            result=timeout(wait,self.request_inner(&id,encoded))=>result.map_err(|_|error("extension bridge request timed out"))?,
            _=cancelled.changed()=>Err(error("extension bridge connection closed")),
        }
    }

    async fn request_inner(&self, id: &str, encoded: String) -> Result<Value, BridgeError> {
        let mut guard = self.socket.lock().await;
        // Own the socket across every await. Cancellation drops it rather than
        // returning an in-flight transaction to the shared connection slot.
        let mut socket = match guard.take() {
            Some(socket) => socket,
            None => self.open().await?,
        };
        send_text(&mut socket, encoded).await?;
        let response = async {
            // An absolute deadline plus a message count bounds unrelated replies.
            for _ in 0..128 {
                let response = receive_json(&mut socket).await?;
                if response.get("id").and_then(Value::as_str) == Some(id) {
                    return Ok(response);
                }
            }
            Err(error("too many unrelated bridge replies"))
        }
        .await?;
        *guard = Some(socket);
        if let Some(value) = response.get("error") {
            return Err(wire_error(value));
        }
        Ok(response.get("result").cloned().unwrap_or(Value::Null))
    }

    /// List accounts, applying Python's metadata fallbacks and malformed-entry skipping.
    pub async fn list_accounts(&self) -> Result<Vec<ExtensionAccount>, BridgeError> {
        accounts_from_result(&self.request("accounts.list", json!({})).await?)
    }

    /// Ask the extension to sign a Polkadot-JS payload, optionally labelling a stage.
    pub async fn sign_extrinsic_payload(
        &self,
        payload: Value,
        stage: Option<&str>,
        more_coming: bool,
    ) -> Result<Value, BridgeError> {
        super::bounded_value(&payload)?;
        if stage.is_some_and(|stage| stage.len() > TEXT_LIMIT) {
            return Err(error("bridge stage label exceeds its size limit"));
        }
        let mut params = json!({"payload": payload, "moreComing": more_coming});
        if let Some(stage) = stage {
            params["stage"] = Value::String(stage.to_owned());
        }
        let result = self.request("extrinsic.sign", params).await?;
        if !result.is_object() {
            return Err(error("bridge returned an invalid extrinsic signature"));
        }
        Ok(result)
    }

    /// Ask the extension's `signRaw` API to sign exact bytes.
    pub async fn sign_bytes(&self, address: &str, data_hex: &str) -> Result<Value, BridgeError> {
        if address.len() > TEXT_LIMIT || data_hex.len() > MESSAGE_LIMIT {
            return Err(error("bridge signing input exceeds its size limit"));
        }
        let result = self
            .request(
                "bytes.sign",
                json!({"address":address,"data":data_hex,"type":"bytes"}),
            )
            .await?;
        if !result.is_object() {
            return Err(error("bridge returned an invalid bytes signature"));
        }
        Ok(result)
    }

    /// Update the bridge page after transaction submission finishes.
    pub async fn report_transaction_result(&self, success: bool) -> Result<(), BridgeError> {
        self.request("transaction.result", json!({"success":success}))
            .await?;
        Ok(())
    }
}

pub(crate) fn validate_url(raw: &str) -> Result<Url, BridgeError> {
    let url = Url::parse(raw).map_err(|_| error("invalid extension bridge URL"))?;
    if !matches!(url.scheme(), "ws" | "wss")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.query().is_some()
        || url.path() != "/ws"
    {
        return Err(error(
            "bridge URL must be a loopback ws:// or wss:// URL ending in /ws",
        ));
    }
    let ip: IpAddr = url
        .host_str()
        .unwrap_or_default()
        .trim_matches(['[', ']'])
        .parse()
        .map_err(|_| error("extension bridge requires a numeric loopback address"))?;
    if !ip.is_loopback() {
        return Err(error("extension bridge requires a loopback address"));
    }
    Ok(url)
}

pub(crate) fn session_url(raw: &str) -> Result<Url, BridgeError> {
    let mut url = Url::parse(raw).map_err(|_| error("invalid extension bridge URL"))?;
    let scheme = match url.scheme() {
        "http" => Some("ws"),
        "https" => Some("wss"),
        _ => None,
    };
    if let Some(scheme) = scheme {
        if url.query().is_some() || url.fragment().is_some() {
            return Err(error("bridge base URL cannot contain a query or fragment"));
        }
        let port = url.port().unwrap_or(super::DEFAULT_BRIDGE_PORT);
        url.set_scheme(scheme)
            .map_err(|_| error("invalid bridge URL scheme"))?;
        url.set_port(Some(port))
            .map_err(|_| error("invalid bridge URL port"))?;
        url.set_path("/ws");
    }
    validate_url(url.as_str())
}

pub(crate) fn target(url: &Url) -> Result<SocketAddr, BridgeError> {
    let ip = url
        .host_str()
        .unwrap_or_default()
        .trim_matches(['[', ']'])
        .parse()
        .map_err(|_| error("invalid loopback address"))?;
    Ok(SocketAddr::new(
        ip,
        url.port_or_known_default()
            .ok_or_else(|| error("bridge URL has no port"))?,
    ))
}

fn tls_config() -> Result<Arc<rustls::ClientConfig>, BridgeError> {
    let mut roots = rustls::RootCertStore::empty();
    let native = rustls_native_certs::load_native_certs();
    for cert in native.certs {
        roots
            .add(cert)
            .map_err(|_| error("invalid system TLS certificate"))?;
    }
    let mut config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|_| error("cannot configure TLS"))?
    .with_root_certificates(roots)
    .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(config))
}

pub(crate) async fn send_text<S>(
    socket: &mut WebSocketStream<S>,
    text: String,
) -> Result<(), BridgeError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    timeout(SEND_TIMEOUT, socket.send(Message::Text(text.into())))
        .await
        .map_err(|_| error("bridge send timed out"))?
        .map_err(|_| error("bridge connection closed"))
}

pub(crate) async fn receive_json<S>(socket: &mut WebSocketStream<S>) -> Result<Value, BridgeError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    for _ in 0..128 {
        match socket.next().await {
            Some(Ok(Message::Text(text))) => return bounded_json(text.as_bytes()),
            Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) => continue,
            _ => return Err(error("bridge connection closed or sent a non-text message")),
        }
    }
    Err(error("too many bridge control messages"))
}

pub(crate) fn accounts_from_result(result: &Value) -> Result<Vec<ExtensionAccount>, BridgeError> {
    let Some(entries) = result.get("accounts").and_then(Value::as_array) else {
        return Ok(Vec::new());
    };
    if entries.len() > 4096 {
        return Err(error("too many extension accounts"));
    }
    let mut accounts = Vec::new();
    for entry in entries {
        let Some(address) = entry.get("address").and_then(Value::as_str) else {
            continue;
        };
        let meta = entry.get("meta");
        let text = |name: &str, fallback: &str| {
            meta.and_then(|meta| meta.get(name))
                .filter(|v| truthy(v))
                .or_else(|| entry.get(name).filter(|v| truthy(v)))
                .map(|v| python_text(v, false))
                .unwrap_or_else(|| fallback.to_owned())
        };
        let account = ExtensionAccount {
            address: address.to_owned(),
            name: text("name", address),
            source: text("source", "unknown"),
            account_type: entry
                .get("type")
                .filter(|v| truthy(v))
                .map(|v| python_text(v, false))
                .unwrap_or_else(|| "sr25519".to_owned()),
        };
        if [
            &account.address,
            &account.name,
            &account.source,
            &account.account_type,
        ]
        .iter()
        .any(|s| s.len() > TEXT_LIMIT)
        {
            return Err(error("extension account field exceeds the size limit"));
        }
        accounts.push(account);
    }
    Ok(accounts)
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64().map(|value| value != 0.0).unwrap_or_else(|| {
            value
                .to_string()
                .split(['e', 'E'])
                .next()
                .is_some_and(|mantissa| mantissa.bytes().any(|b| matches!(b, b'1'..=b'9')))
        }),
        Value::String(value) => !value.is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
    }
}
fn python_text(value: &Value, nested: bool) -> String {
    match value {
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::String(value) if !nested => value.clone(),
        Value::String(value) => rtao_primitives::pyfmt::string_repr(value),
        Value::Number(value) => {
            let text = value.to_string();
            if text.contains(['.', 'e', 'E']) {
                rtao_primitives::pyfmt::float_repr(text.parse::<f64>().unwrap_or(f64::NAN))
            } else {
                text
            }
        }
        Value::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(|value| python_text(value, true))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Object(values) => format!(
            "{{{}}}",
            values
                .iter()
                .map(|(key, value)| format!(
                    "{}: {}",
                    python_text(&Value::String(key.clone()), true),
                    python_text(value, true)
                ))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}
