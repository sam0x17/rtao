// Derived from RaoFoundation/subtensor sdk/python/bittensor/evm/rpc.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Bounded asynchronous Ethereum JSON-RPC over HTTP.
//!
//! Requests, including raw transaction submissions, are sent once. A timeout
//! or transport failure after submission does not establish that the node
//! rejected the transaction; callers can reconcile its locally computed hash.

use std::{
    io::{self, Write},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use alloy_primitives::{Address, B256, U256, keccak256};
use reqwest::redirect::Policy;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Value, json};
use tokio::time::{Instant, sleep, timeout};
use url::Url;

pub use crate::money::{WEI_PER_RAO, WEI_PER_TAO, balance_to_wei, wei_to_balance};

use crate::{Error, Result};

const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;

/// Resource limits for one endpoint and its receipt polling.
#[derive(Clone, Debug)]
pub struct RpcOptions {
    /// Total wall time for a request, including all response-body chunks.
    pub request_timeout: Duration,
    /// Maximum serialized JSON request size, at most 16 MiB.
    pub max_request_bytes: usize,
    /// Maximum JSON response size, at most 16 MiB.
    pub max_response_bytes: usize,
    /// Delay between receipt requests; the pinned Python default is 3 seconds.
    pub receipt_poll_interval: Duration,
}

impl Default for RpcOptions {
    fn default() -> Self {
        Self {
            request_timeout: Duration::from_secs(30),
            max_request_bytes: MAX_BODY_BYTES,
            max_response_bytes: MAX_BODY_BYTES,
            receipt_poll_interval: Duration::from_secs(3),
        }
    }
}

/// Fields used by the client from a mined Ethereum transaction receipt.
///
/// The complete response remains available in `raw`, including logs and
/// chain-specific fields. A failed EVM execution is a receipt with
/// `success == false`, rather than a transport error.
#[derive(Clone, Debug, PartialEq)]
pub struct TransactionReceipt {
    /// Hash of the transaction whose receipt was requested.
    pub transaction_hash: B256,
    /// Canonical block height reported by the receipt.
    pub block_number: u64,
    /// Gas consumed by this transaction.
    pub gas_used: u64,
    /// Whether the EVM status field is exactly one.
    pub success: bool,
    /// Address created by a deployment, if any.
    pub contract_address: Option<Address>,
    /// Unmodified JSON receipt, including fields not interpreted here.
    pub raw: Value,
}

/// Cloneable JSON-RPC client with shared, monotonically increasing request IDs.
#[derive(Clone)]
pub struct EvmRpc {
    http: reqwest::Client,
    url: Url,
    options: RpcOptions,
    next_id: Arc<AtomicU64>,
}

impl EvmRpc {
    /// Construct an endpoint with native TLS roots, explicit ring and defaults.
    pub fn new(url: impl AsRef<str>) -> Result<Self> {
        Self::with_options(url, RpcOptions::default())
    }

    /// Construct an endpoint with explicit request and buffering limits.
    ///
    /// TLS uses an owned ring configuration and does not change the process
    /// default crypto provider. Redirects and automatic retries are disabled.
    pub fn with_options(url: impl AsRef<str>, options: RpcOptions) -> Result<Self> {
        let mut roots = rustls::RootCertStore::empty();
        roots.add_parsable_certificates(rustls_native_certs::load_native_certs().certs);
        let tls = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .map_err(|error| Error::Validation(format!("invalid TLS configuration: {error}")))?
        .with_root_certificates(roots)
        .with_no_client_auth();
        Self::with_tls_config(url, options, tls)
    }

    /// Construct with an owned TLS configuration, for custom trust roots.
    ///
    /// Callers constructing a custom configuration should explicitly select
    /// its crypto provider. HTTP/2 and HTTP/1.1 ALPN are set by this method.
    pub fn with_tls_config(
        url: impl AsRef<str>,
        options: RpcOptions,
        mut tls: rustls::ClientConfig,
    ) -> Result<Self> {
        if options.request_timeout.is_zero()
            || Instant::now()
                .checked_add(options.request_timeout)
                .is_none()
            || options.receipt_poll_interval.is_zero()
            || !(1..=MAX_BODY_BYTES).contains(&options.max_request_bytes)
            || !(1..=MAX_BODY_BYTES).contains(&options.max_response_bytes)
        {
            return Err(Error::Validation(
                "EVM RPC requires positive timeouts and request/response bounds of 1..=16777216 bytes".into(),
            ));
        }
        let url = Url::parse(url.as_ref())
            .map_err(|_| Error::Validation("invalid EVM RPC endpoint URL".into()))?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
        {
            return Err(Error::Validation(
                "EVM RPC endpoint requires HTTP(S), a host and no credentials or fragment".into(),
            ));
        }
        tls.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
        let http = reqwest::Client::builder()
            .tls_backend_preconfigured(tls)
            .redirect(Policy::none())
            .retry(reqwest::retry::never())
            .connect_timeout(options.request_timeout)
            .timeout(options.request_timeout)
            .build()
            .map_err(|error| {
                Error::Transport(format!("could not construct EVM RPC client: {error}"))
            })?;
        Ok(Self {
            http,
            url,
            options,
            next_id: Arc::new(AtomicU64::new(0)),
        })
    }

    /// Endpoint URL, including any explicitly supplied path and query.
    pub fn url(&self) -> &Url {
        &self.url
    }

    /// Resource and polling limits for this client.
    pub fn options(&self) -> &RpcOptions {
        &self.options
    }

    /// Send one JSON-RPC request, validating version, correlation and envelope.
    ///
    /// Responses with a mismatched ID, duplicate envelope keys, both result
    /// and error, or neither field are rejected. JSON-RPC error code, message
    /// and data are preserved. This method never retries a request.
    pub async fn request(&self, method: &str, params: &[Value]) -> Result<Value> {
        if method.is_empty() || method.len() > 128 || method.chars().any(char::is_control) {
            return Err(Error::Validation("invalid EVM RPC method name".into()));
        }
        let id = self
            .next_id
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_add(1)
            })
            .map_err(|_| Error::Validation("EVM RPC request IDs exhausted".into()))?
            + 1;
        let request = Request {
            jsonrpc: "2.0",
            id,
            method,
            params,
        };
        let mut writer = LimitedWriter::new(self.options.max_request_bytes);
        if let Err(error) = serde_json::to_writer(&mut writer, &request) {
            return if writer.overflow {
                Err(Error::Validation(
                    "EVM RPC request exceeds configured size limit".into(),
                ))
            } else {
                Err(Error::Json(error))
            };
        }
        timeout(
            self.options.request_timeout,
            self.exchange(id, writer.bytes),
        )
        .await
        .map_err(|_| Error::Timeout(format!("EVM RPC {method} request timed out")))?
    }

    /// Alias matching the pinned Python client's raw `call` method.
    pub async fn call(&self, method: &str, params: &[Value]) -> Result<Value> {
        self.request(method, params).await
    }

    async fn exchange(&self, id: u64, body: Vec<u8>) -> Result<Value> {
        let mut response = self
            .http
            .post(self.url.clone())
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body)
            .send()
            .await
            .map_err(transport_error)?;
        if !response.status().is_success() {
            return Err(Error::Transport(format!(
                "EVM RPC returned HTTP {}",
                response.status()
            )));
        }
        if response
            .content_length()
            .is_some_and(|length| length > self.options.max_response_bytes as u64)
        {
            return Err(Error::Validation(
                "EVM RPC response exceeds configured size limit".into(),
            ));
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(transport_error)? {
            if chunk.len() > self.options.max_response_bytes.saturating_sub(body.len()) {
                return Err(Error::Validation(
                    "EVM RPC response exceeds configured size limit".into(),
                ));
            }
            body.extend_from_slice(&chunk);
        }
        let envelope: Envelope = serde_json::from_slice(&body)?;
        if envelope.jsonrpc != "2.0" || envelope.id != id {
            return Err(Error::Validation(
                "EVM RPC response version or request ID mismatch".into(),
            ));
        }
        match (envelope.result, envelope.error) {
            (Present::Value(value), Present::Absent) => Ok(value),
            (Present::Absent, Present::Value(value)) => {
                let error: RpcError = serde_json::from_value(value)?;
                Err(Error::Rpc {
                    code: error.code,
                    message: error.message,
                    data: error.data,
                })
            }
            _ => Err(Error::Validation(
                "EVM RPC response must contain exactly one of result or error".into(),
            )),
        }
    }

    /// Current on-chain EVM chain ID.
    pub async fn chain_id(&self) -> Result<u64> {
        quantity_u64(&self.request("eth_chainId", &[]).await?)
    }

    /// Current EVM block height.
    pub async fn block_number(&self) -> Result<u64> {
        quantity_u64(&self.request("eth_blockNumber", &[]).await?)
    }

    /// EVM account balance in wei at the latest block.
    pub async fn get_balance_wei(&self, address: Address) -> Result<U256> {
        quantity_u256(
            &self
                .request(
                    "eth_getBalance",
                    &[json!(format!("{address:#x}")), json!("latest")],
                )
                .await?,
        )
    }

    /// Transaction count at the latest block, matching the pinned Python API.
    pub async fn get_nonce(&self, address: Address) -> Result<u64> {
        quantity_u64(
            &self
                .request(
                    "eth_getTransactionCount",
                    &[json!(format!("{address:#x}")), json!("latest")],
                )
                .await?,
        )
    }

    /// Suggested gas price in wei per gas unit.
    pub async fn gas_price(&self) -> Result<U256> {
        quantity_u256(&self.request("eth_gasPrice", &[]).await?)
    }

    /// Estimate gas for an Ethereum transaction object.
    pub async fn estimate_gas(&self, transaction: &Value) -> Result<u64> {
        require_object(transaction)?;
        quantity_u64(
            &self
                .request("eth_estimateGas", std::slice::from_ref(transaction))
                .await?,
        )
    }

    /// Execute a read-only EVM call at the latest block and decode its bytes.
    pub async fn eth_call(&self, transaction: &Value) -> Result<Vec<u8>> {
        require_object(transaction)?;
        data_bytes(
            &self
                .request("eth_call", &[transaction.clone(), json!("latest")])
                .await?,
        )
    }

    /// Submit a signed transaction exactly once and check its returned hash.
    ///
    /// The expected hash is `keccak256(raw)`, available before transmission.
    /// Every failure after invoking the request preserves that exact hash in
    /// [`Error::UnknownFate`]. Reconcile it before deciding to submit again.
    pub async fn send_raw_transaction(&self, raw: &[u8]) -> Result<B256> {
        if raw.is_empty() || raw.len() > self.options.max_request_bytes / 2 {
            return Err(Error::Validation(
                "raw EVM transaction is empty or exceeds the request size limit".into(),
            ));
        }
        let expected = keccak256(raw);
        let value = self
            .request(
                "eth_sendRawTransaction",
                &[json!(format!("0x{}", hex::encode(raw)))],
            )
            .await
            .map_err(|cause| unknown_fate(expected, cause))?;
        let actual = hash_value(&value).map_err(|cause| unknown_fate(expected, cause))?;
        if actual != expected {
            return Err(unknown_fate(
                expected,
                Error::Validation(format!(
                    "EVM RPC returned the wrong submitted transaction hash; expected {expected:#x}"
                )),
            ));
        }
        Ok(actual)
    }

    /// Decode a hex-encoded signed transaction and submit it exactly once.
    pub async fn send_raw_transaction_hex(&self, raw: &str) -> Result<B256> {
        if raw.len() > self.options.max_request_bytes {
            return Err(Error::Validation(
                "raw EVM transaction exceeds the request size limit".into(),
            ));
        }
        self.send_raw_transaction(&data_bytes(&Value::String(raw.into()))?)
            .await
    }

    /// Fetch and validate a transaction receipt, returning `None` while pending.
    ///
    /// A failed observation retains the requested hash in [`Error::UnknownFate`].
    pub async fn get_receipt(&self, transaction_hash: B256) -> Result<Option<TransactionReceipt>> {
        let raw = self
            .request(
                "eth_getTransactionReceipt",
                &[json!(format!("{transaction_hash:#x}"))],
            )
            .await
            .map_err(|cause| unknown_fate(transaction_hash, cause))?;
        if raw.is_null() {
            return Ok(None);
        }
        decode_receipt(transaction_hash, raw)
            .map(Some)
            .map_err(|cause| unknown_fate(transaction_hash, cause))
    }

    /// Poll until a receipt arrives, with a deadline covering requests and sleeps.
    ///
    /// Each request is bounded by the smaller of its own timeout and the
    /// remaining receipt deadline. Expiry returns [`Error::ReceiptTimeout`];
    /// other observation failures retain the hash in [`Error::UnknownFate`].
    /// Only a successful `null` receipt causes another poll.
    pub async fn wait_for_receipt(
        &self,
        transaction_hash: B256,
        wait: Duration,
    ) -> Result<TransactionReceipt> {
        if wait.is_zero() {
            return Err(Error::ReceiptTimeout(transaction_hash));
        }
        let deadline = Instant::now().checked_add(wait).ok_or_else(|| {
            unknown_fate(
                transaction_hash,
                Error::Validation("receipt timeout exceeds clock range".into()),
            )
        })?;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(Error::ReceiptTimeout(transaction_hash));
            }
            match timeout(remaining, self.get_receipt(transaction_hash)).await {
                Ok(Ok(Some(receipt))) => return Ok(receipt),
                Ok(Ok(None)) => {}
                Ok(Err(_)) if Instant::now() >= deadline => {
                    return Err(Error::ReceiptTimeout(transaction_hash));
                }
                Ok(Err(error)) => return Err(error),
                Err(_) => return Err(Error::ReceiptTimeout(transaction_hash)),
            }
            sleep(
                self.options
                    .receipt_poll_interval
                    .min(deadline.saturating_duration_since(Instant::now())),
            )
            .await;
        }
    }
}

fn unknown_fate(hash: B256, cause: Error) -> Error {
    if cause.unknown_fate_hash() == Some(hash) {
        cause
    } else {
        Error::UnknownFate {
            hash,
            cause: Box::new(cause),
        }
    }
}

fn transport_error(error: reqwest::Error) -> Error {
    if error.is_timeout() {
        Error::Timeout("EVM RPC request timed out".into())
    } else {
        // Avoid copying an endpoint's query credentials into diagnostics.
        Error::Transport(format!("EVM RPC transport failed: {}", error.without_url()))
    }
}

#[derive(Serialize)]
struct Request<'a> {
    jsonrpc: &'static str,
    id: u64,
    method: &'a str,
    params: &'a [Value],
}

#[derive(Default)]
enum Present {
    #[default]
    Absent,
    Value(Value),
}

impl<'de> Deserialize<'de> for Present {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        Value::deserialize(deserializer).map(Self::Value)
    }
}

#[derive(Deserialize)]
struct Envelope {
    jsonrpc: String,
    id: u64,
    #[serde(default)]
    result: Present,
    #[serde(default)]
    error: Present,
}

#[derive(Deserialize)]
struct RpcError {
    code: i64,
    message: String,
    data: Option<Value>,
}

struct LimitedWriter {
    bytes: Vec<u8>,
    limit: usize,
    overflow: bool,
}

impl LimitedWriter {
    fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::new(),
            limit,
            overflow: false,
        }
    }
}

impl Write for LimitedWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            self.overflow = true;
            return Err(io::Error::other("JSON request exceeds limit"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub(crate) fn quantity_u256(value: &Value) -> Result<U256> {
    let hex = value
        .as_str()
        .and_then(|text| text.strip_prefix("0x"))
        .ok_or_else(|| Error::Validation("EVM quantity requires a 0x-prefixed string".into()))?;
    if hex.is_empty()
        || hex.len() > 64
        || (hex.len() > 1 && hex.starts_with('0'))
        || !hex.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(Error::Validation(
            "EVM quantity is noncanonical or exceeds 256 bits".into(),
        ));
    }
    U256::from_str_radix(hex, 16)
        .map_err(|_| Error::Validation("EVM quantity exceeds 256 bits".into()))
}

pub(crate) fn quantity_u64(value: &Value) -> Result<u64> {
    let quantity = quantity_u256(value)?;
    u64::try_from(quantity).map_err(|_| Error::Validation("EVM quantity exceeds u64".into()))
}

fn data_bytes(value: &Value) -> Result<Vec<u8>> {
    let hex = value
        .as_str()
        .and_then(|text| text.strip_prefix("0x"))
        .ok_or_else(|| Error::Validation("EVM data requires a 0x-prefixed string".into()))?;
    if !hex.len().is_multiple_of(2) {
        return Err(Error::Validation(
            "EVM data must contain whole bytes".into(),
        ));
    }
    hex::decode(hex).map_err(|_| Error::Validation("EVM data contains invalid hex".into()))
}

fn hash_value(value: &Value) -> Result<B256> {
    let text = value
        .as_str()
        .filter(|text| text.len() == 66)
        .and_then(|text| text.strip_prefix("0x"))
        .ok_or_else(|| Error::Validation("EVM transaction hash must contain 32 bytes".into()))?;
    let mut bytes = [0u8; 32];
    hex::decode_to_slice(text, &mut bytes)
        .map_err(|_| Error::Validation("invalid EVM transaction hash hex".into()))?;
    Ok(B256::from(bytes))
}

fn require_object(value: &Value) -> Result<()> {
    if value.is_object() {
        Ok(())
    } else {
        Err(Error::Validation(
            "EVM transaction must be an object".into(),
        ))
    }
}

fn receipt_field<'a>(raw: &'a Value, key: &str) -> Result<&'a Value> {
    raw.get(key)
        .ok_or_else(|| Error::Validation(format!("EVM receipt is missing {key}")))
}

fn decode_receipt(expected: B256, raw: Value) -> Result<TransactionReceipt> {
    require_object(&raw)?;
    let transaction_hash = hash_value(receipt_field(&raw, "transactionHash")?)?;
    if transaction_hash != expected {
        return Err(Error::Validation(
            "EVM receipt transaction hash mismatch".into(),
        ));
    }
    if let Some(block_hash) = raw.get("blockHash") {
        hash_value(block_hash)?;
    }
    let block_number = quantity_u64(receipt_field(&raw, "blockNumber")?)?;
    let gas_used = quantity_u64(receipt_field(&raw, "gasUsed")?)?;
    let status = quantity_u64(receipt_field(&raw, "status")?)?;
    if status > 1 {
        return Err(Error::Validation(
            "EVM receipt status must be zero or one".into(),
        ));
    }
    let contract_address = match raw.get("contractAddress") {
        None | Some(Value::Null) => None,
        Some(value) => {
            let bytes = data_bytes(value)?;
            if bytes.len() != 20 {
                return Err(Error::Validation(
                    "EVM receipt contract address must contain 20 bytes".into(),
                ));
            }
            Some(Address::from_slice(&bytes))
        }
    };
    Ok(TransactionReceipt {
        transaction_hash,
        block_number,
        gas_used,
        success: status == 1,
        contract_address,
        raw,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::{TcpListener, TcpStream},
        task::JoinHandle,
    };

    use super::*;

    enum Payload {
        Result(Value),
        Error(Value),
        Envelope(Value),
        Raw(String),
    }

    struct Reply {
        payload: Payload,
        status: u16,
        chunked: bool,
        delay: Duration,
    }

    impl Reply {
        fn result(value: Value) -> Self {
            Self {
                payload: Payload::Result(value),
                status: 200,
                chunked: false,
                delay: Duration::ZERO,
            }
        }

        fn envelope(value: Value) -> Self {
            Self {
                payload: Payload::Envelope(value),
                ..Self::result(Value::Null)
            }
        }
    }

    async fn read_request(stream: &mut TcpStream) -> Value {
        let mut bytes = Vec::new();
        let mut next = [0u8; 1024];
        let (header_end, length) = loop {
            let count = stream.read(&mut next).await.unwrap();
            assert_ne!(count, 0);
            bytes.extend_from_slice(&next[..count]);
            assert!(bytes.len() < 65536);
            if let Some(position) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                let headers = std::str::from_utf8(&bytes[..position]).unwrap();
                let length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().unwrap())
                    })
                    .unwrap();
                break (position + 4, length);
            }
        };
        while bytes.len() < header_end + length {
            let count = stream.read(&mut next).await.unwrap();
            assert_ne!(count, 0);
            bytes.extend_from_slice(&next[..count]);
        }
        serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap()
    }

    async fn script(replies: Vec<Reply>) -> (String, JoinHandle<Vec<Value>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let mut requests = Vec::new();
            for reply in replies {
                let (mut stream, _) = timeout(Duration::from_secs(5), listener.accept())
                    .await
                    .unwrap()
                    .unwrap();
                let request = timeout(Duration::from_secs(5), read_request(&mut stream))
                    .await
                    .unwrap();
                let body = match reply.payload {
                    Payload::Result(value) => serde_json::to_vec(
                        &json!({"jsonrpc":"2.0", "id":request["id"], "result":value}),
                    )
                    .unwrap(),
                    Payload::Error(value) => serde_json::to_vec(
                        &json!({"jsonrpc":"2.0", "id":request["id"], "error":value}),
                    )
                    .unwrap(),
                    Payload::Envelope(value) => serde_json::to_vec(&value).unwrap(),
                    Payload::Raw(value) => value.into_bytes(),
                };
                requests.push(request);
                sleep(reply.delay).await;
                let reason = if reply.status == 200 { "OK" } else { "Error" };
                let response = if reply.chunked {
                    format!("HTTP/1.1 {} {}\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:x}\r\n{}\r\n0\r\n\r\n", reply.status, reason, body.len(), String::from_utf8(body).unwrap()).into_bytes()
                } else {
                    let mut response = format!(
                        "HTTP/1.1 {} {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        reply.status,
                        reason,
                        body.len()
                    )
                    .into_bytes();
                    response.extend_from_slice(&body);
                    response
                };
                // A deliberate timeout or body-limit rejection can close first.
                let _ = stream.write_all(&response).await;
            }
            requests
        });
        (format!("http://{address}"), task)
    }

    fn options() -> RpcOptions {
        RpcOptions {
            receipt_poll_interval: Duration::from_millis(1),
            ..RpcOptions::default()
        }
    }

    fn receipt(hash: B256, status: &str) -> Value {
        json!({
            "transactionHash":format!("{hash:#x}"), "blockNumber":"0xa",
            "gasUsed":"0x5208", "status":status,
            "contractAddress":null, "logs":[{"data":"0xbeef"}],
        })
    }

    #[test]
    fn quantities_require_canonical_hex_and_check_native_boundaries() {
        assert_eq!(quantity_u64(&json!("0x0")).unwrap(), 0);
        assert_eq!(
            quantity_u64(&json!("0xffffffffffffffff")).unwrap(),
            u64::MAX
        );
        assert_eq!(
            quantity_u256(&json!(format!("0x{}", "f".repeat(64)))).unwrap(),
            U256::MAX
        );
        for invalid in [
            json!(0),
            json!("1"),
            json!("0x"),
            json!("0x00"),
            json!("0x01"),
            json!("0X1"),
            json!("0xg"),
            json!("0x-1"),
            json!(format!("0x1{}", "0".repeat(64))),
        ] {
            assert!(quantity_u256(&invalid).is_err(), "{invalid}");
        }
        assert!(quantity_u64(&json!("0x10000000000000000")).is_err());
        assert_eq!(data_bytes(&json!("0x")).unwrap(), Vec::<u8>::new());
        for invalid in [json!("0x0"), json!("ab"), json!("0xz1")] {
            assert!(data_bytes(&invalid).is_err());
        }
    }

    #[tokio::test]
    async fn typed_methods_send_exact_pinned_parameters_and_share_ids() {
        let raw = [0xde, 0xad];
        let expected_hash = keccak256(raw);
        let (url, task) = script(vec![
            Reply::result(json!("0x3c4")),
            Reply::result(json!("0x10")),
            Reply::result(json!("0x10000000000000000")),
            Reply::result(json!("0x7")),
            Reply::result(json!("0x3b9aca00")),
            Reply::result(json!("0x5208")),
            Reply::result(json!("0x0001ab")),
            Reply::result(json!(format!("{expected_hash:#x}"))),
        ])
        .await;
        let rpc = EvmRpc::new(url).unwrap();
        let address = Address::from([0x11; 20]);
        assert_eq!(rpc.chain_id().await.unwrap(), 964);
        assert_eq!(rpc.clone().block_number().await.unwrap(), 16);
        assert_eq!(
            rpc.get_balance_wei(address).await.unwrap(),
            U256::from(1u128 << 64)
        );
        assert_eq!(rpc.get_nonce(address).await.unwrap(), 7);
        assert_eq!(rpc.gas_price().await.unwrap(), U256::from(1_000_000_000u64));
        let tx = json!({"to":format!("{address:#x}"), "data":"0x1234"});
        assert_eq!(rpc.estimate_gas(&tx).await.unwrap(), 21000);
        assert_eq!(rpc.eth_call(&tx).await.unwrap(), [0, 1, 0xab]);
        assert_eq!(rpc.send_raw_transaction(&raw).await.unwrap(), expected_hash);
        let requests = task.await.unwrap();
        for (index, request) in requests.iter().enumerate() {
            assert_eq!(request["id"], index + 1);
            assert_eq!(request["jsonrpc"], "2.0");
        }
        assert_eq!(
            requests
                .iter()
                .map(|request| request["method"].as_str().unwrap())
                .collect::<Vec<_>>(),
            [
                "eth_chainId",
                "eth_blockNumber",
                "eth_getBalance",
                "eth_getTransactionCount",
                "eth_gasPrice",
                "eth_estimateGas",
                "eth_call",
                "eth_sendRawTransaction"
            ]
        );
        assert_eq!(requests[0]["params"], json!([]));
        assert_eq!(
            requests[2]["params"],
            json!([format!("{address:#x}"), "latest"])
        );
        assert_eq!(
            requests[3]["params"],
            json!([format!("{address:#x}"), "latest"])
        );
        assert_eq!(requests[5]["params"], json!([tx]));
        assert_eq!(requests[6]["params"], json!([tx, "latest"]));
        assert_eq!(requests[7]["params"], json!(["0xdead"]));
    }

    #[tokio::test]
    async fn invalid_envelopes_and_error_shapes_are_rejected() {
        for payload in [
            Payload::Envelope(json!({"jsonrpc":"2.0", "id":2, "result":"0x1"})),
            Payload::Envelope(json!({"jsonrpc":"1.0", "id":1, "result":"0x1"})),
            Payload::Envelope(json!({"jsonrpc":"2.0", "id":"1", "result":"0x1"})),
            Payload::Envelope(json!({"jsonrpc":"2.0", "id":1})),
            Payload::Envelope(
                json!({"jsonrpc":"2.0", "id":1, "result":null,"error":{"code":-1,"message":"bad"}}),
            ),
            Payload::Envelope(
                json!({"jsonrpc":"2.0", "id":1, "error":{"code":"-1","message":"bad"}}),
            ),
            Payload::Envelope(json!({"jsonrpc":"2.0", "id":1, "error":null})),
            Payload::Raw("{\"jsonrpc\":\"2.0\",\"id\":1,\"id\":1,\"result\":true}".into()),
            Payload::Raw("{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":true,\"result\":false}".into()),
        ] {
            let (url, task) = script(vec![Reply {
                payload,
                ..Reply::result(Value::Null)
            }])
            .await;
            assert!(
                EvmRpc::new(url)
                    .unwrap()
                    .request("eth_chainId", &[])
                    .await
                    .is_err()
            );
            assert_eq!(task.await.unwrap().len(), 1);
        }
        let (url, task) = script(vec![Reply {
            payload: Payload::Error(
                json!({"code":-32000,"message":"execution reverted","data":{"revert":"0xbeef"}}),
            ),
            ..Reply::result(Value::Null)
        }])
        .await;
        let error = EvmRpc::new(url)
            .unwrap()
            .request("eth_call", &[])
            .await
            .unwrap_err();
        match error {
            Error::Rpc {
                code,
                message,
                data,
            } => {
                assert_eq!(code, -32000);
                assert_eq!(message, "execution reverted");
                assert_eq!(data, Some(json!({"revert":"0xbeef"})));
            }
            other => panic!("wrong error: {other:?}"),
        }
        task.await.unwrap();
    }

    #[tokio::test]
    async fn receipts_poll_only_null_preserve_failure_and_check_hashes() {
        let hash = B256::from([0x22; 32]);
        let mut deployed = receipt(hash, "0x1");
        deployed["contractAddress"] = json!(format!("0x{}", "ab".repeat(20)));
        let (url, task) = script(vec![
            Reply::result(Value::Null),
            Reply::result(deployed.clone()),
            Reply::result(receipt(hash, "0x0")),
        ])
        .await;
        let rpc = EvmRpc::with_options(url, options()).unwrap();
        let result = rpc
            .wait_for_receipt(hash, Duration::from_secs(2))
            .await
            .unwrap();
        assert_eq!(result.transaction_hash, hash);
        assert_eq!(result.block_number, 10);
        assert_eq!(result.gas_used, 21000);
        assert!(result.success);
        assert_eq!(result.contract_address, Some(Address::from([0xab; 20])));
        assert_eq!(result.raw, deployed);
        assert!(!rpc.get_receipt(hash).await.unwrap().unwrap().success);
        let requests = task.await.unwrap();
        assert_eq!(requests.len(), 3);
        for request in requests {
            assert_eq!(request["method"], "eth_getTransactionReceipt");
            assert_eq!(request["params"], json!([format!("{hash:#x}")]));
        }
        for malformed in [
            receipt(B256::ZERO, "0x1"),
            receipt(hash, "0x2"),
            {
                let mut value = receipt(hash, "0x1");
                value["gasUsed"] = json!("0x10000000000000000");
                value
            },
            {
                let mut value = receipt(hash, "0x1");
                value["blockNumber"] = Value::Null;
                value
            },
            {
                let mut value = receipt(hash, "0x1");
                value["contractAddress"] = json!("0x01");
                value
            },
            {
                let mut value = receipt(hash, "0x1");
                value["blockHash"] = json!("0x00");
                value
            },
        ] {
            assert!(decode_receipt(hash, malformed).is_err());
        }
    }

    #[tokio::test]
    async fn every_submission_response_failure_retains_the_exact_attempted_hash() {
        let raw = [1, 2, 3];
        let expected = keccak256(raw);
        for reply in [
            Reply::result(json!(format!("{:#x}", B256::ZERO))),
            Reply::result(json!("0x00")),
            Reply {
                payload: Payload::Raw("not json".into()),
                ..Reply::result(Value::Null)
            },
            Reply {
                payload: Payload::Error(
                    json!({"code":-32000,"message":"rejected","data":"details"}),
                ),
                ..Reply::result(Value::Null)
            },
            Reply {
                status: 503,
                ..Reply::result(Value::Null)
            },
        ] {
            let (url, task) = script(vec![reply]).await;
            let error = EvmRpc::new(url)
                .unwrap()
                .send_raw_transaction(&raw)
                .await
                .unwrap_err();
            assert_eq!(error.unknown_fate_hash(), Some(expected));
            match error {
                Error::UnknownFate { hash, cause } => {
                    assert_eq!(hash, expected);
                    assert!(cause.unknown_fate_hash().is_none());
                }
                other => panic!("submission lost its attempted hash: {other:?}"),
            }
            let requests = task.await.unwrap();
            assert_eq!(requests.len(), 1);
            assert_eq!(requests[0]["params"], json!(["0x010203"]));
        }
        for hash in [
            json!("0x00"),
            json!("1"),
            json!(format!("0x{}", "gg".repeat(32))),
        ] {
            assert!(hash_value(&hash).is_err());
        }
    }

    #[tokio::test]
    async fn advertised_and_chunked_response_limits_and_request_bounds_are_enforced() {
        for chunked in [false, true] {
            let (url, task) = script(vec![Reply {
                chunked,
                ..Reply::result(json!("x".repeat(512)))
            }])
            .await;
            let rpc = EvmRpc::with_options(
                url,
                RpcOptions {
                    max_response_bytes: 128,
                    ..options()
                },
            )
            .unwrap();
            assert!(matches!(
                rpc.request("test", &[]).await,
                Err(Error::Validation(_))
            ));
            assert_eq!(task.await.unwrap().len(), 1);
        }
        let rpc = EvmRpc::with_options(
            "http://127.0.0.1:1",
            RpcOptions {
                max_request_bytes: 128,
                ..options()
            },
        )
        .unwrap();
        assert!(matches!(
            rpc.request("test", &[json!("x".repeat(1024))]).await,
            Err(Error::Validation(_))
        ));
        assert!(matches!(
            rpc.estimate_gas(&json!([])).await,
            Err(Error::Validation(_))
        ));
        assert!(matches!(
            rpc.send_raw_transaction(&[]).await,
            Err(Error::Validation(_))
        ));
        let raw = [1; 40];
        let error = rpc.send_raw_transaction(&raw).await.unwrap_err();
        assert_eq!(error.unknown_fate_hash(), Some(keccak256(raw)));
        assert!(matches!(error, Error::UnknownFate { .. }));
    }

    #[tokio::test]
    async fn pending_receipt_expiry_and_invalid_direct_receipt_retain_requested_hash() {
        let hash = B256::from([0x55; 32]);
        let no_request = EvmRpc::new("http://127.0.0.1:1").unwrap();
        let overflow = no_request
            .wait_for_receipt(hash, Duration::MAX)
            .await
            .unwrap_err();
        assert_eq!(overflow.unknown_fate_hash(), Some(hash));
        assert!(matches!(overflow, Error::UnknownFate { .. }));
        let (url, task) = script(vec![Reply::result(Value::Null)]).await;
        let rpc = EvmRpc::with_options(
            url,
            RpcOptions {
                receipt_poll_interval: Duration::from_secs(1),
                ..options()
            },
        )
        .unwrap();
        let error = rpc
            .wait_for_receipt(hash, Duration::from_millis(50))
            .await
            .unwrap_err();
        assert!(matches!(error, Error::ReceiptTimeout(actual) if actual == hash));
        assert_eq!(task.await.unwrap().len(), 1);

        let (url, task) = script(vec![Reply::result(receipt(B256::ZERO, "0x1"))]).await;
        let error = EvmRpc::new(url)
            .unwrap()
            .get_receipt(hash)
            .await
            .unwrap_err();
        assert_eq!(error.unknown_fate_hash(), Some(hash));
        match error {
            Error::UnknownFate { cause, .. } => assert!(matches!(*cause, Error::Validation(_))),
            other => panic!("direct receipt lost its requested hash: {other:?}"),
        }
        assert_eq!(task.await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn receipt_deadline_covers_an_inflight_request_and_errors_do_not_poll() {
        let hash = B256::ZERO;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let (accepted_tx, accepted_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_request(&mut stream).await;
            accepted_tx.send(()).unwrap();
            release_rx.await.unwrap();
            request
        });
        let rpc = EvmRpc::with_options(url, options()).unwrap();
        let waiting =
            tokio::spawn(
                async move { rpc.wait_for_receipt(hash, Duration::from_millis(30)).await },
            );
        accepted_rx.await.unwrap();
        let error = timeout(Duration::from_secs(5), waiting)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert_eq!(error.unknown_fate_hash(), Some(hash));
        assert!(matches!(error, Error::ReceiptTimeout(actual) if actual == hash));
        release_tx.send(()).unwrap();
        assert_eq!(task.await.unwrap()["method"], "eth_getTransactionReceipt");
        let (url, task) = script(vec![Reply {
            payload: Payload::Error(json!({"code":-1,"message":"broken"})),
            ..Reply::result(Value::Null)
        }])
        .await;
        let rpc = EvmRpc::new(url).unwrap();
        let error = rpc
            .wait_for_receipt(hash, Duration::from_secs(1))
            .await
            .unwrap_err();
        assert_eq!(error.unknown_fate_hash(), Some(hash));
        match error {
            Error::UnknownFate {
                hash: actual,
                cause,
            } => {
                assert_eq!(actual, hash);
                assert!(matches!(*cause, Error::Rpc { code: -1, .. }));
            }
            other => panic!("receipt observation lost its attempted hash: {other:?}"),
        }
        let error = rpc
            .wait_for_receipt(hash, Duration::ZERO)
            .await
            .unwrap_err();
        assert!(matches!(error, Error::ReceiptTimeout(actual) if actual == hash));
        assert_eq!(task.await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_disconnected_submission_is_never_retried() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_request(&mut stream).await;
            assert_eq!(request["method"], "eth_sendRawTransaction");
            drop(stream);
            assert!(
                timeout(Duration::from_millis(100), listener.accept())
                    .await
                    .is_err()
            );
        });
        let raw = [0xf8, 0x01];
        let error = EvmRpc::new(url)
            .unwrap()
            .send_raw_transaction(&raw)
            .await
            .unwrap_err();
        assert_eq!(error.unknown_fate_hash(), Some(keccak256(raw)));
        assert!(matches!(error, Error::UnknownFate { .. }));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn http_errors_and_redirects_return_without_replay() {
        for status in [302, 429, 503] {
            let (url, task) = script(vec![Reply {
                status,
                ..Reply::result(Value::Null)
            }])
            .await;
            assert!(matches!(
                EvmRpc::new(url).unwrap().request("eth_chainId", &[]).await,
                Err(Error::Transport(_))
            ));
            assert_eq!(task.await.unwrap().len(), 1);
        }
    }
}
