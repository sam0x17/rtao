// Derived from opentensor/bittensor bittensor/core/dendrite.py @ v10.5.0 (MIT); modified by rtao.
//! A reqwest v10 dendrite.
//!
//! Requests carry Python's header set and order, a Python-spaced JSON body
//! and `content-type: application/json`. Responses merge into the local
//! envelope exactly as Python's `process_server_response` does. Transport
//! failures map by category: connect → 503, deadline → 408, body or decode →
//! 422, anything else → 500, each with Python's message text.

use std::{fmt, sync::Arc, time::Duration, time::Instant};

use bytes::Bytes;
use futures::{StreamExt, stream::BoxStream};
use num_bigint::BigInt;
use rtao_btauth::{Clock, SystemClock};
use rtao_primitives::{axon::AxonTarget, pyfmt::float_repr};
use rtao_signer::Signer;
use serde_json::Value;
use tokio::sync::oneshot;
use url::Url;

use crate::{
    CodecError, Envelope, Synapse, TerminalInfo, V10_VERSION_AS_INT, codec::response_terminals,
    json::to_python_json, uuid1_string, v10_message,
};

/// Which uuid a request carries.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UuidMode {
    /// One uuid per dendrite, as Python. Python axons keep one nonce per
    /// `hotkey:uuid`, so concurrent calls that arrive out of order get 401.
    #[default]
    PerInstance,
    /// A fresh uuid per request: no out-of-order 401s, at the cost of one
    /// nonce-table entry per request on Python axons.
    PerRequest,
}

/// Per-call options.
#[derive(Clone, Debug, PartialEq)]
pub struct CallOptions {
    /// Total deadline, also sent as `timeout` (default 12 s).
    pub timeout: Duration,
    /// `bt_header_dendrite_version` (default 10005000).
    pub version: i64,
    /// Uuid selection.
    pub uuid_mode: UuidMode,
    /// Python sends to `0.0.0.0` when the target IP equals the dendrite's
    /// external IP. Off by default.
    pub rewrite_self_ip: bool,
}

impl Default for CallOptions {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(12),
            version: V10_VERSION_AS_INT,
            uuid_mode: UuidMode::PerInstance,
            rewrite_self_ip: false,
        }
    }
}

/// A request could not be built or signed.
#[derive(Debug)]
#[non_exhaustive]
pub enum DendriteError {
    /// The HTTP client could not be configured.
    Configuration(String),
    /// The synapse could not be encoded.
    Codec(CodecError),
    /// The signer or clock failed.
    Signing(String),
    /// The target URL is unusable.
    Target(String),
    /// A streamed body failed after the response started.
    Transport(String),
}

impl fmt::Display for DendriteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Configuration(message)
            | Self::Signing(message)
            | Self::Target(message)
            | Self::Transport(message) => formatter.write_str(message),
            Self::Codec(error) => error.fmt(formatter),
        }
    }
}
impl std::error::Error for DendriteError {}
impl From<CodecError> for DendriteError {
    fn from(error: CodecError) -> Self {
        Self::Codec(error)
    }
}

/// A signed request ready to send, exposed for inspection and testing.
#[derive(Clone, Debug)]
pub struct PreparedRequest<S> {
    /// Endpoint `http://host:port/NAME`.
    pub url: Url,
    /// The envelope as signed and sent.
    pub envelope: Envelope<S>,
    /// Headers in Python's order (without `content-type`).
    pub headers: Vec<(String, String)>,
    /// Python-spaced JSON body.
    pub body: Vec<u8>,
    /// The signed message.
    pub message: String,
}

/// A v10 dendrite: one HTTP pool, one signing hotkey and one uuid.
#[derive(Clone)]
pub struct Dendrite {
    client: reqwest::Client,
    signer: Arc<dyn Signer>,
    hotkey: String,
    uuid: String,
    external_ip: Option<String>,
    clock: Arc<dyn Clock>,
    max_response_bytes: usize,
}

impl fmt::Debug for Dendrite {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Dendrite")
            .field("hotkey", &self.hotkey)
            .field("uuid", &self.uuid)
            .finish_non_exhaustive()
    }
}

struct Failure {
    status: i64,
    message: String,
}

impl Dendrite {
    /// A dendrite for a signer that supports raw signing.
    ///
    /// Hardware signers refuse raw signing, so every call then fails with 422.
    pub fn new(signer: Arc<dyn Signer>) -> Result<Self, DendriteError> {
        let tls = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .map_err(|error| DendriteError::Configuration(error.to_string()))?
        .with_root_certificates(rustls::RootCertStore::empty())
        .with_no_client_auth();
        // aiohttp's ClientSession ignores proxy variables (trust_env=False);
        // proxies would also see the signed headers in plain HTTP and often
        // drop underscore header names.
        let client = reqwest::Client::builder()
            .tls_backend_preconfigured(tls)
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .build()
            .map_err(|error| DendriteError::Configuration(error.to_string()))?;
        Ok(Self {
            hotkey: signer.account().to_string(),
            signer,
            client,
            uuid: uuid1_string(),
            external_ip: None,
            clock: Arc::new(SystemClock::default()),
            max_response_bytes: 64 * 1024 * 1024,
        })
    }

    /// Report this IP in `bt_header_dendrite_ip` (Python: the external IP).
    pub fn with_external_ip(mut self, ip: impl Into<String>) -> Self {
        self.external_ip = Some(ip.into());
        self
    }

    /// Use a fixed instance uuid (Python: `uuid1()` per dendrite).
    pub fn with_uuid(mut self, uuid: impl Into<String>) -> Self {
        self.uuid = uuid.into();
        self
    }

    /// Use another nonce clock.
    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    /// Retain at most this many response bytes (default 64 MiB), including
    /// error bodies of streaming calls. Streamed 200 bodies are not retained
    /// and are not limited.
    pub fn with_max_response_bytes(mut self, bytes: usize) -> Self {
        self.max_response_bytes = bytes.max(1);
        self
    }

    /// The signing hotkey's SS58 text.
    pub fn hotkey(&self) -> &str {
        &self.hotkey
    }

    /// This instance's uuid.
    pub fn uuid(&self) -> &str {
        &self.uuid
    }

    /// The underlying HTTP client.
    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }

    /// Build and sign a request without sending it.
    pub async fn prepare<S: Synapse>(
        &self,
        target: &AxonTarget,
        body: S,
        options: &CallOptions,
    ) -> Result<PreparedRequest<S>, DendriteError> {
        const {
            assert!(
                crate::is_valid_synapse_name(S::NAME),
                "Synapse::NAME must be an ASCII Python identifier"
            );
        }
        let host = target
            .base_url
            .host_str()
            .ok_or_else(|| DendriteError::Target("axon URL has no host".into()))?;
        let ip = host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_owned();
        let port = target
            .base_url
            .port_or_known_default()
            .ok_or_else(|| DendriteError::Target("axon URL has no port".into()))?;
        let uuid = match options.uuid_mode {
            UuidMode::PerInstance => self.uuid.clone(),
            UuidMode::PerRequest => uuid1_string(),
        };
        let nonce = self
            .clock
            .now_ns()
            .map_err(|error| DendriteError::Signing(error.to_string()))?;
        let nonce_i64 =
            i64::try_from(nonce).map_err(|_| DendriteError::Signing("nonce exceeds i64".into()))?;
        let receiver = target.hotkey.to_string();

        let mut envelope = Envelope::new(body);
        envelope.timeout = Some(options.timeout.as_secs_f64());
        envelope.dendrite = Some(TerminalInfo {
            ip: self.external_ip.clone(),
            version: Some(options.version),
            nonce: Some(nonce_i64),
            uuid: Some(uuid.clone()),
            hotkey: Some(self.hotkey.clone()),
            ..TerminalInfo::default()
        });
        envelope.axon = Some(TerminalInfo {
            ip: Some(ip.clone()),
            port: Some(i64::from(port)),
            hotkey: Some(receiver.clone()),
            ..TerminalInfo::default()
        });
        if envelope.py_dump()?.has_non_finite() {
            return Err(DendriteError::Codec(CodecError::Unsupported(
                "non-finite floats are not valid JSON".into(),
            )));
        }
        let hash = envelope.body_hash()?;
        let message = v10_message(
            &BigInt::from(nonce),
            &self.hotkey,
            &receiver,
            Some(&uuid),
            Some(&hash),
        );
        let signature = self
            .signer
            .sign_raw(message.as_bytes())
            .await
            .map_err(|error| DendriteError::Signing(error.to_string()))?;
        if let Some(dendrite) = envelope.dendrite.as_mut() {
            dendrite.signature = Some(format!("0x{}", hex::encode(signature)));
        }
        let draft = to_python_json(&envelope)
            .map_err(|error| DendriteError::Codec(CodecError::Body(error.to_string())))?;
        envelope.total_size = i64::try_from(draft.len()).ok();
        let headers = envelope.header_pairs()?;
        let body = to_python_json(&envelope)
            .map_err(|error| DendriteError::Codec(CodecError::Body(error.to_string())))?;
        let mut base = target.base_url.clone();
        if options.rewrite_self_ip && self.external_ip.as_deref() == Some(ip.as_str()) {
            base.set_host(Some("0.0.0.0"))
                .map_err(|error| DendriteError::Target(error.to_string()))?;
        }
        let url = Url::parse(&format!(
            "{}/{}",
            base.as_str().trim_end_matches('/'),
            S::NAME
        ))
        .map_err(|error| DendriteError::Target(error.to_string()))?;
        Ok(PreparedRequest {
            url,
            envelope,
            headers,
            body,
            message,
        })
    }

    fn request<S>(
        &self,
        prepared: &PreparedRequest<S>,
        options: &CallOptions,
    ) -> reqwest::RequestBuilder {
        let mut request = self
            .client
            .post(prepared.url.clone())
            .timeout(options.timeout);
        for (name, value) in &prepared.headers {
            request = request.header(name.as_str(), value.as_str());
        }
        request
            .header(http::header::CONTENT_TYPE, "application/json")
            .body(prepared.body.clone())
    }

    /// Python's `Dendrite.call`: always returns the envelope, with
    /// `dendrite.status_code`/`status_message` describing the outcome.
    pub async fn call<S: Synapse>(
        &self,
        target: &AxonTarget,
        body: S,
        options: &CallOptions,
    ) -> Envelope<S> {
        let start = Instant::now();
        match self.prepare(target, body.clone(), options).await {
            Ok(prepared) => self.send_prepared(prepared, options, start).await,
            Err(error) => {
                let mut envelope = Envelope::new(body);
                envelope.timeout = Some(options.timeout.as_secs_f64());
                fail(
                    &mut envelope,
                    Failure {
                        status: 422,
                        message: format!("Failed to parse response: {error}"),
                    },
                );
                envelope
            }
        }
    }

    /// Send a prepared (possibly modified) request and merge the response.
    pub async fn send_prepared<S: Synapse>(
        &self,
        prepared: PreparedRequest<S>,
        options: &CallOptions,
        start: Instant,
    ) -> Envelope<S> {
        let request = self.request(&prepared, options);
        let mut envelope = prepared.envelope;
        let outcome = async {
            let mut response = request
                .send()
                .await
                .map_err(|error| transport(&envelope, &error))?;
            let status = response.status().as_u16();
            let headers = response.headers().clone();
            let body = read_capped(&mut response, self.max_response_bytes, &envelope).await?;
            let json = response_json(&headers, &body)?;
            merge_response(&mut envelope, status, Some((&json, &body)), &headers)
        }
        .await;
        match outcome {
            Ok(()) => {
                if let Some(dendrite) = envelope.dendrite.as_mut() {
                    dendrite.process_time = Some(start.elapsed().as_secs_f64());
                }
            }
            Err(failure) => fail(&mut envelope, failure),
        }
        envelope
    }

    /// Python's `forward` over several targets, in target order, with at
    /// most `concurrency` requests in flight.
    pub async fn forward<S: Synapse>(
        &self,
        targets: &[AxonTarget],
        body: S,
        options: &CallOptions,
        concurrency: usize,
    ) -> Vec<Envelope<S>> {
        futures::stream::iter(targets)
            .map(|target| self.call(target, body.clone(), options))
            .buffered(concurrency.max(1))
            .collect()
            .await
    }

    /// Python's `call_stream`: raw response chunks, then the final envelope.
    ///
    /// A 200 response streams its body; the final envelope merges only the
    /// response headers (Python subclasses build that JSON from headers too).
    /// Any other status reads the body as `{"message": …}` and yields nothing.
    pub fn call_stream<S: Synapse>(
        &self,
        target: &AxonTarget,
        body: S,
        options: &CallOptions,
    ) -> (
        BoxStream<'static, Result<Bytes, DendriteError>>,
        oneshot::Receiver<Envelope<S>>,
    ) {
        let (sender, receiver) = oneshot::channel();
        let state = StreamState::Start {
            dendrite: self.clone(),
            target: target.clone(),
            body,
            options: options.clone(),
            sender,
        };
        let stream = futures::stream::unfold(state, |state| async move { state.next().await });
        (stream.boxed(), receiver)
    }
}

#[allow(clippy::large_enum_variant)]
enum StreamState<S> {
    Start {
        dendrite: Dendrite,
        target: AxonTarget,
        body: S,
        options: CallOptions,
        sender: oneshot::Sender<Envelope<S>>,
    },
    Body {
        response: reqwest::Response,
        envelope: Envelope<S>,
        sender: oneshot::Sender<Envelope<S>>,
        start: Instant,
    },
    Done,
}

impl<S: Synapse> StreamState<S> {
    async fn next(self) -> Option<(Result<Bytes, DendriteError>, Self)> {
        match self {
            Self::Done => None,
            Self::Start {
                dendrite,
                target,
                body,
                options,
                sender,
            } => {
                let start = Instant::now();
                let prepared = match dendrite.prepare(&target, body.clone(), &options).await {
                    Ok(prepared) => prepared,
                    Err(error) => {
                        let mut envelope = Envelope::new(body);
                        fail(
                            &mut envelope,
                            Failure {
                                status: 422,
                                message: format!("Failed to parse response: {error}"),
                            },
                        );
                        let _ = sender.send(envelope);
                        return None;
                    }
                };
                let request = dendrite.request(&prepared, &options);
                let mut envelope = prepared.envelope;
                let response = match request.send().await {
                    Ok(response) => response,
                    Err(error) => {
                        let failure = transport(&envelope, &error);
                        fail(&mut envelope, failure);
                        let _ = sender.send(envelope);
                        return None;
                    }
                };
                if response.status().as_u16() != 200 {
                    let mut response = response;
                    let status = response.status().as_u16();
                    let headers = response.headers().clone();
                    let outcome =
                        match read_capped(&mut response, dendrite.max_response_bytes, &envelope)
                            .await
                        {
                            Ok(body) => response_json(&headers, &body).and_then(|json| {
                                merge_response(
                                    &mut envelope,
                                    status,
                                    Some((&json, &body)),
                                    &headers,
                                )
                            }),
                            Err(failure) => Err(failure),
                        };
                    if let Err(failure) = outcome {
                        fail(&mut envelope, failure);
                    }
                    let _ = sender.send(envelope);
                    return None;
                }
                Box::pin(
                    Self::Body {
                        response,
                        envelope,
                        sender,
                        start,
                    }
                    .next(),
                )
                .await
            }
            Self::Body {
                mut response,
                mut envelope,
                sender,
                start,
            } => match response.chunk().await {
                Ok(Some(chunk)) => Some((
                    Ok(chunk),
                    Self::Body {
                        response,
                        envelope,
                        sender,
                        start,
                    },
                )),
                Ok(None) => {
                    let headers = response.headers().clone();
                    match merge_response(&mut envelope, 200, None, &headers) {
                        Ok(()) => {
                            if let Some(dendrite) = envelope.dendrite.as_mut() {
                                dendrite.process_time = Some(start.elapsed().as_secs_f64());
                            }
                        }
                        Err(failure) => fail(&mut envelope, failure),
                    }
                    let _ = sender.send(envelope);
                    None
                }
                Err(error) => {
                    let failure = transport(&envelope, &error);
                    let text = failure.message.clone();
                    fail(&mut envelope, failure);
                    let _ = sender.send(envelope);
                    Some((Err(DendriteError::Transport(text)), Self::Done))
                }
            },
        }
    }
}

/// Read a whole response body, refusing more than `limit` bytes.
async fn read_capped<S: Synapse>(
    response: &mut reqwest::Response,
    limit: usize,
    envelope: &Envelope<S>,
) -> Result<Vec<u8>, Failure> {
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| transport(envelope, &error))?
    {
        if body.len() + chunk.len() > limit {
            return Err(parse_failure(format!("response exceeds {limit} bytes")));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn parse_failure(message: impl fmt::Display) -> Failure {
    Failure {
        status: 422,
        message: format!("Failed to parse response: {message}"),
    }
}

fn transport<S: Synapse>(envelope: &Envelope<S>, error: &reqwest::Error) -> Failure {
    if error.is_timeout() {
        let timeout = envelope
            .timeout
            .map_or_else(|| "None".to_owned(), float_repr);
        return Failure {
            status: 408,
            message: format!("Request timeout after {timeout} seconds"),
        };
    }
    if error.is_connect() {
        let axon = envelope.axon.clone().unwrap_or_default();
        let ip = axon.ip.unwrap_or_else(|| "None".into());
        let port = axon
            .port
            .map_or_else(|| "None".to_owned(), |port| port.to_string());
        return Failure {
            status: 503,
            message: format!("Service unavailable at {ip}:{port}/{}", S::NAME),
        };
    }
    if error.is_body() || error.is_decode() {
        return parse_failure(error);
    }
    Failure {
        status: 500,
        message: format!("Client error: {error}"),
    }
}

fn fail<S>(envelope: &mut Envelope<S>, failure: Failure) {
    let dendrite = envelope.dendrite.get_or_insert_with(TerminalInfo::default);
    dendrite.status_code = Some(failure.status);
    dendrite.status_message = Some(failure.message);
}

/// aiohttp's `response.json()`: a JSON content type is required and an
/// empty body yields `None`, which Python then fails to merge.
fn response_json(headers: &http::HeaderMap, body: &[u8]) -> Result<Value, Failure> {
    let content_type = headers
        .get(http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    let json_type = content_type
        .strip_prefix("application/")
        .is_some_and(|rest| {
            rest.starts_with("json")
                || rest.split_once("+json").is_some_and(|(subtype, _)| {
                    !subtype.is_empty() && !subtype.contains([';', ' '])
                })
        });
    if !json_type {
        return Err(parse_failure(format!(
            "Attempt to decode JSON with unexpected mimetype: {content_type}"
        )));
    }
    if body.iter().all(u8::is_ascii_whitespace) {
        return Err(parse_failure("empty response body"));
    }
    serde_json::from_slice(body).map_err(parse_failure)
}

/// Python's `process_server_response`.
fn merge_response<S: Synapse>(
    envelope: &mut Envelope<S>,
    status: u16,
    json: Option<(&Value, &[u8])>,
    headers: &http::HeaderMap,
) -> Result<(), Failure> {
    if status == 200 {
        if let Some((_, body)) = json {
            let server = Envelope::<S>::from_json_slice(body).map_err(parse_failure)?;
            envelope.merge_success(server).map_err(parse_failure)?;
        }
    } else {
        let message = match json.map(|(value, _)| value) {
            Some(Value::Object(map)) => match map.get("message") {
                None | Some(Value::Null) => None,
                Some(Value::String(text)) => Some(text.clone()),
                Some(_) => return Err(parse_failure("status_message is not a string")),
            },
            Some(_) => return Err(parse_failure("response JSON is not an object")),
            None => None,
        };
        let axon = envelope.axon.get_or_insert_with(TerminalInfo::default);
        axon.status_code = Some(i64::from(status));
        axon.status_message = message;
    }
    let (server_dendrite, server_axon) = response_terminals(headers).map_err(parse_failure)?;
    let dendrite = envelope.dendrite.get_or_insert_with(TerminalInfo::default);
    dendrite.merge_present(&server_dendrite);
    let axon = envelope.axon.get_or_insert_with(TerminalInfo::default);
    axon.merge_present(&server_axon);
    let (code, message) = (axon.status_code, axon.status_message.clone());
    let dendrite = envelope.dendrite.get_or_insert_with(TerminalInfo::default);
    dendrite.status_code = code;
    dendrite.status_message = message;
    Ok(())
}
