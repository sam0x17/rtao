// Derived from RaoFoundation/subtensor sdk/python/bittensor/_transport/rpc.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Retained handshake transport and bounded HTTP refusal diagnostics.
//! Capture keeps at most 4096 decoded bytes, processes at most 64 KiB of
//! post-header wire data and caps each chunk line at 1024 bytes. The original
//! absolute handshake deadline includes optional body acquisition. Known HTTP
//! status and metadata survive optional failures; malformed framing has no detail.
use crate::{RpcError, SessionConfig, generated::py_rpc, policy};
use std::{
    io,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, ReadBuf},
    net::TcpStream,
    time::{Instant, timeout_at},
};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, client_async_with_config,
    tungstenite::{
        Error as WsError,
        client::IntoClientRequest,
        error::{TlsError, UrlError},
        handshake::client::Request,
        protocol::WebSocketConfig,
    },
};
use url::Url;

pub(super) type Socket = WebSocketStream<RetainedIo<MaybeTlsStream<TcpStream>>>;
const BODY_WIRE_LIMIT: usize = 64 * 1024;
const CHUNK_LINE_LIMIT: usize = 1024;
type Lease<S> = Arc<Mutex<Option<S>>>;

/// Only the handshake uses a shared lease; success transfers direct ownership.
pub(super) enum RetainedIo<S> {
    Shared(Lease<S>),
    Owned(S),
}
impl<S> RetainedIo<S> {
    fn promote(&mut self) -> io::Result<()> {
        let stream = match self {
            Self::Shared(lease) => take(lease)?,
            Self::Owned(_) => return Ok(()),
        };
        *self = Self::Owned(stream);
        Ok(())
    }
    fn with_stream<T>(
        &mut self,
        poll: impl FnOnce(&mut S) -> Poll<io::Result<T>>,
    ) -> Poll<io::Result<T>> {
        match self {
            Self::Owned(stream) => poll(stream),
            Self::Shared(lease) => match lease.lock() {
                Ok(mut stream) => match stream.as_mut() {
                    Some(stream) => poll(stream),
                    None => Poll::Ready(Err(io::Error::other("RPC handshake stream is absent"))),
                },
                Err(_) => Poll::Ready(Err(io::Error::other("RPC handshake stream lock poisoned"))),
            },
        }
    }
}
fn take<S>(lease: &Lease<S>) -> io::Result<S> {
    lease
        .lock()
        .map_err(|_| io::Error::other("RPC handshake stream lock poisoned"))?
        .take()
        .ok_or_else(|| io::Error::other("RPC handshake stream is absent"))
}
impl<S: AsyncRead + Unpin> AsyncRead for RetainedIo<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        self.get_mut()
            .with_stream(|stream| Pin::new(stream).poll_read(cx, buf))
    }
}
impl<S: AsyncWrite + Unpin> AsyncWrite for RetainedIo<S> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.get_mut()
            .with_stream(|stream| Pin::new(stream).poll_write(cx, bytes))
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.get_mut()
            .with_stream(|stream| Pin::new(stream).poll_flush(cx))
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.get_mut()
            .with_stream(|stream| Pin::new(stream).poll_shutdown(cx))
    }
}

#[derive(Debug)]
enum UpgradeError {
    Timeout,
    WebSocket(WsError),
}

// The same generic helper is exercised with explicitly fragmented test I/O.
async fn upgrade<S: AsyncRead + AsyncWrite + Unpin>(
    request: Request,
    stream: S,
    config: WebSocketConfig,
    deadline: Instant,
) -> Result<WebSocketStream<RetainedIo<S>>, UpgradeError> {
    let lease = Arc::new(Mutex::new(Some(stream)));
    let result = timeout_at(
        deadline,
        client_async_with_config(request, RetainedIo::Shared(lease.clone()), Some(config)),
    )
    .await
    .map_err(|_| UpgradeError::Timeout)?;
    match result {
        Ok((mut socket, _)) => {
            // Keep the actual verified WebSocket and its buffered 101 tail.
            socket
                .get_mut()
                .promote()
                .map_err(|error| UpgradeError::WebSocket(WsError::Io(error)))?;
            Ok(socket)
        }
        Err(WsError::Http(mut response)) => {
            let tail = response.body_mut().take().unwrap_or_default();
            let prefix = match take(&lease) {
                Ok(stream) => {
                    refusal_prefix(
                        stream,
                        tail,
                        response.status().as_u16(),
                        response.headers(),
                        deadline,
                    )
                    .await
                }
                // An optional diagnostic must never overwrite a known HTTP status.
                Err(_) => Vec::new(),
            };
            *response.body_mut() = Some(prefix);
            Err(UpgradeError::WebSocket(WsError::Http(response)))
        }
        Err(error) => Err(UpgradeError::WebSocket(error)),
    }
}

pub(super) async fn connect_socket(
    config: &SessionConfig,
    url: &Url,
    tls: Arc<rustls::ClientConfig>,
) -> Result<Socket, RpcError> {
    let mut request = url
        .as_str()
        .into_client_request()
        .map_err(|error| RpcError::Protocol(error.to_string()))?;
    for (name, value) in &config.headers {
        let mut value = value.clone();
        value.set_sensitive(true);
        request.headers_mut().append(name, value);
    }
    let websocket = WebSocketConfig::default()
        .max_message_size(Some(config.max_message_bytes))
        .max_frame_size(Some(config.max_message_bytes));
    let deadline = Instant::now()
        .checked_add(config.handshake_timeout)
        .ok_or_else(|| RpcError::Protocol("RPC handshake timeout is too large".into()))?;
    let timed_out = || RpcError::Timeout {
        detail: format!(
            "no response during the WebSocket handshake with {} after {:.0}s",
            policy::safe_endpoint(url.as_str()),
            config.handshake_timeout.as_secs_f64()
        ),
    };
    let stream = timeout_at(deadline, transport(&request, tls))
        .await
        .map_err(|_| timed_out())?
        .map_err(transport_error)?;
    match upgrade(request, stream, websocket, deadline).await {
        Ok(socket) => Ok(socket),
        Err(UpgradeError::Timeout) => Err(timed_out()),
        Err(UpgradeError::WebSocket(WsError::Http(response))) => {
            let status = response.status().as_u16();
            let body = response.body().as_deref();
            if let Some(policy) = policy::classify_handshake(status, response.headers(), body) {
                Err(RpcError::Policy(Box::new(policy)))
            } else {
                Err(RpcError::Handshake {
                    status,
                    detail: body.map(policy::response_detail).unwrap_or_default(),
                })
            }
        }
        Err(UpgradeError::WebSocket(error)) => Err(transport_error(error)),
    }
}
fn transport_error(error: WsError) -> RpcError {
    RpcError::Transport(policy::safe_diagnostic(
        &error.to_string(),
        py_rpc::POLICY_DETAIL_LIMIT,
    ))
}
async fn transport(
    request: &Request,
    tls: Arc<rustls::ClientConfig>,
) -> Result<MaybeTlsStream<TcpStream>, WsError> {
    let host = request
        .uri()
        .host()
        .ok_or(WsError::Url(UrlError::NoHostName))?;
    // Match the existing connector's IPv6 ServerName handling.
    let host = host
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or(host);
    let secure = match request.uri().scheme_str() {
        Some("wss") => true,
        Some("ws") => false,
        _ => return Err(WsError::Url(UrlError::UnsupportedUrlScheme)),
    };
    let port = request
        .uri()
        .port_u16()
        .unwrap_or(if secure { 443 } else { 80 });
    let tcp = TcpStream::connect((host, port)).await?;
    if !secure {
        return Ok(MaybeTlsStream::Plain(tcp));
    }
    let name = rustls_pki_types::ServerName::try_from(host.to_owned())
        .map_err(|_| WsError::Tls(TlsError::InvalidDnsName))?;
    let stream = tokio_rustls::TlsConnector::from(tls)
        .connect(name, tcp)
        .await?;
    Ok(MaybeTlsStream::Rustls(stream))
}

#[derive(Clone, Copy)]
enum Framing {
    Length(usize),
    Chunked,
    Eof,
}
#[derive(Clone, Copy)]
enum CaptureError {
    Malformed,
    Limit,
    Incomplete,
}
fn framing(headers: &http::HeaderMap) -> Result<Framing, CaptureError> {
    let mut lengths = headers.get_all(http::header::CONTENT_LENGTH).iter();
    let length = lengths.next();
    let mut codings = headers.get_all(http::header::TRANSFER_ENCODING).iter();
    let coding = codings.next();
    if lengths.next().is_some()
        || codings.next().is_some()
        || (length.is_some() && coding.is_some())
    {
        return Err(CaptureError::Malformed);
    }
    if let Some(coding) = coding {
        return if trim_ows(coding.as_bytes()).eq_ignore_ascii_case(b"chunked") {
            Ok(Framing::Chunked)
        } else {
            Err(CaptureError::Malformed)
        };
    }
    if let Some(length) = length {
        let bytes = trim_ows(length.as_bytes());
        if bytes.is_empty() || !bytes.iter().all(u8::is_ascii_digit) {
            return Err(CaptureError::Malformed);
        }
        let length = bytes.iter().fold(0_usize, |value, digit| {
            value
                .saturating_mul(10)
                .saturating_add(usize::from(digit - b'0'))
                .min(py_rpc::POLICY_BODY_LIMIT)
        });
        return Ok(Framing::Length(length));
    }
    Ok(Framing::Eof)
}
fn trim_ows(mut bytes: &[u8]) -> &[u8] {
    while bytes
        .first()
        .is_some_and(|byte| matches!(*byte, b' ' | b'\t'))
    {
        bytes = &bytes[1..];
    }
    while bytes
        .last()
        .is_some_and(|byte| matches!(*byte, b' ' | b'\t'))
    {
        bytes = &bytes[..bytes.len() - 1];
    }
    bytes
}
// Validate extension syntax as bytes arrive, including before EOF/deadline.
// State advances once per byte and never allocates beyond the bounded line.
enum ChunkLineState {
    Size(bool),
    AfterSize,
    NameStart,
    Name,
    AfterName,
    ValueStart,
    Value,
    Quoted,
    Escape,
    AfterValue,
}
impl ChunkLineState {
    fn token(byte: u8) -> bool {
        byte.is_ascii_alphanumeric()
            || matches!(
                byte,
                0x21 | 0x23..=0x27 | 0x2a | 0x2b | 0x2d | 0x2e | 0x5e | 0x5f | 0x60 | 0x7c | 0x7e
            )
    }
    fn advance(&mut self, byte: u8) -> Result<(), CaptureError> {
        let ows = byte == 9 || byte == 32;
        *self = match self {
            Self::Size(_) if byte.is_ascii_hexdigit() => Self::Size(true),
            Self::Size(true) if ows => Self::AfterSize,
            Self::AfterSize if ows => Self::AfterSize,
            Self::Size(true) | Self::AfterSize if byte == b';' => Self::NameStart,
            Self::NameStart if ows => Self::NameStart,
            Self::NameStart if Self::token(byte) => Self::Name,
            Self::Name if Self::token(byte) => Self::Name,
            Self::Name | Self::AfterName if byte == b'=' => Self::ValueStart,
            Self::Name | Self::AfterName | Self::Value | Self::AfterValue if byte == b';' => {
                Self::NameStart
            }
            Self::Name | Self::AfterName if ows => Self::AfterName,
            Self::ValueStart if ows => Self::ValueStart,
            Self::ValueStart if byte == b'"' => Self::Quoted,
            Self::ValueStart | Self::Value if Self::token(byte) => Self::Value,
            Self::Value | Self::AfterValue if ows => Self::AfterValue,
            Self::Quoted if byte == b'"' => Self::AfterValue,
            Self::Quoted if byte == 0x5c => Self::Escape,
            Self::Quoted
                if byte == 9
                    || byte == 32
                    || byte == 0x21
                    || (0x23..=0x5b).contains(&byte)
                    || (0x5d..=0x7e).contains(&byte)
                    || byte >= 0x80 =>
            {
                Self::Quoted
            }
            Self::Escape
                if byte == 9 || byte == 32 || (0x21..=0x7e).contains(&byte) || byte >= 0x80 =>
            {
                Self::Quoted
            }
            _ => return Err(CaptureError::Malformed),
        };
        Ok(())
    }
    fn complete(&self) -> bool {
        matches!(
            self,
            Self::Size(true) | Self::Name | Self::AfterName | Self::Value | Self::AfterValue
        )
    }
}
async fn refusal_prefix<S: AsyncRead + Unpin>(
    stream: S,
    tail: Vec<u8>,
    status: u16,
    headers: &http::HeaderMap,
    deadline: Instant,
) -> Vec<u8> {
    if status / 100 == 1 || status == 204 || status == 304 {
        return Vec::new();
    }
    let Ok(framing) = framing(headers) else {
        return Vec::new();
    };
    let mut reader = BodyReader {
        stream,
        tail,
        position: 0,
        processed: 0,
    };
    let mut prefix = Vec::with_capacity(py_rpc::POLICY_BODY_LIMIT);
    // Keep decoded progress outside the timed future so expiry cannot erase it.
    if matches!(
        timeout_at(deadline, reader.capture(framing, &mut prefix)).await,
        Ok(Err(CaptureError::Malformed))
    ) {
        prefix.clear();
    }
    prefix
}
struct BodyReader<S> {
    stream: S,
    tail: Vec<u8>,
    position: usize,
    processed: usize,
}
impl<S: AsyncRead + Unpin> BodyReader<S> {
    async fn read(&mut self, output: &mut [u8]) -> Result<usize, CaptureError> {
        let remaining = BODY_WIRE_LIMIT.saturating_sub(self.processed);
        if remaining == 0 {
            return Err(CaptureError::Limit);
        }
        let length = output.len().min(remaining);
        let count = if self.position < self.tail.len() {
            let count = length.min(self.tail.len() - self.position);
            output[..count].copy_from_slice(&self.tail[self.position..self.position + count]);
            self.position += count;
            count
        } else {
            self.stream
                .read(&mut output[..length])
                .await
                .map_err(|_| CaptureError::Incomplete)?
        };
        self.processed += count;
        Ok(count)
    }
    async fn data(&mut self, wanted: usize, prefix: &mut Vec<u8>) -> Result<(), CaptureError> {
        let end = prefix.len() + wanted;
        let mut bytes = [0_u8; py_rpc::POLICY_BODY_LIMIT];
        while prefix.len() < end {
            let count = self.read(&mut bytes[..end - prefix.len()]).await?;
            if count == 0 {
                return Err(CaptureError::Incomplete);
            }
            prefix.extend_from_slice(&bytes[..count]);
        }
        Ok(())
    }
    async fn line(&mut self) -> Result<Vec<u8>, CaptureError> {
        let mut line = Vec::new();
        let mut syntax = ChunkLineState::Size(false);
        loop {
            let mut byte = [0_u8; 1];
            if self.read(&mut byte).await? == 0 {
                return Err(CaptureError::Incomplete);
            }
            line.push(byte[0]);
            if byte[0] == 13 {
                if !syntax.complete() {
                    return Err(CaptureError::Malformed);
                }
            } else if byte[0] != 10 {
                syntax.advance(byte[0])?;
            }
            if line.ends_with(b"\r\n") {
                line.truncate(line.len() - 2);
                return Ok(line);
            }
            if byte[0] == b'\n'
                || line.len() == CHUNK_LINE_LIMIT
                || (line.len() >= 2 && line[line.len() - 2] == b'\r')
            {
                return Err(CaptureError::Malformed);
            }
        }
    }
    async fn capture(
        &mut self,
        framing: Framing,
        prefix: &mut Vec<u8>,
    ) -> Result<(), CaptureError> {
        match framing {
            Framing::Length(length) => self.data(length, prefix).await,
            Framing::Eof => {
                let mut bytes = [0_u8; py_rpc::POLICY_BODY_LIMIT];
                while prefix.len() < py_rpc::POLICY_BODY_LIMIT {
                    let count = self
                        .read(&mut bytes[..py_rpc::POLICY_BODY_LIMIT - prefix.len()])
                        .await?;
                    if count == 0 {
                        break;
                    }
                    prefix.extend_from_slice(&bytes[..count]);
                }
                Ok(())
            }
            Framing::Chunked => loop {
                let line = self.line().await?;
                let extension = line
                    .iter()
                    .position(|byte| *byte == b';')
                    .unwrap_or(line.len());
                let size = trim_ows(&line[..extension]);
                if size.is_empty() || !size.iter().all(u8::is_ascii_hexdigit) {
                    return Err(CaptureError::Malformed);
                }
                let mut length = 0_u64;
                for &digit in size {
                    let value = match digit {
                        b'0'..=b'9' => digit - b'0',
                        b'a'..=b'f' => digit - b'a' + 10,
                        b'A'..=b'F' => digit - b'A' + 10,
                        _ => return Err(CaptureError::Malformed),
                    };
                    length = length
                        .checked_mul(16)
                        .and_then(|length| length.checked_add(u64::from(value)))
                        .ok_or(CaptureError::Malformed)?;
                }
                if length == 0 {
                    return Ok(());
                }
                let wanted = usize::try_from(length)
                    .unwrap_or(usize::MAX)
                    .min(py_rpc::POLICY_BODY_LIMIT - prefix.len());
                self.data(wanted, prefix).await?;
                if prefix.len() == py_rpc::POLICY_BODY_LIMIT {
                    return Ok(());
                }
                for expected in b"\r\n" {
                    let mut byte = [0_u8; 1];
                    if self.read(&mut byte).await? == 0 {
                        return Err(CaptureError::Incomplete);
                    }
                    // Reject known malformed framing before waiting for more.
                    if byte[0] != *expected {
                        return Err(CaptureError::Malformed);
                    }
                }
            },
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod body_tests {
    use super::*;
    fn headers(name: &str, value: &str) -> http::HeaderMap {
        let mut headers = http::HeaderMap::new();
        headers.insert(
            http::header::HeaderName::from_bytes(name.as_bytes()).unwrap(),
            value.parse().unwrap(),
        );
        headers
    }
    async fn capture(headers: &http::HeaderMap, body: Vec<u8>) -> Vec<u8> {
        refusal_prefix(
            tokio::io::empty(),
            body,
            429,
            headers,
            Instant::now() + std::time::Duration::from_secs(1),
        )
        .await
    }
    #[tokio::test]
    async fn decoded_prefix_limits_and_many_tiny_chunks_use_the_production_reader() {
        for size in [4095, 4096, 4097] {
            let body = vec![b'x'; size];
            assert_eq!(
                capture(&headers("content-length", &size.to_string()), body).await,
                vec![b'x'; size.min(4096)]
            );
        }
        let mut chunks = b"1\r\nx\r\n".repeat(4096);
        chunks.extend_from_slice(b"0\r\nignored trailers");
        assert_eq!(
            capture(&headers("transfer-encoding", "CHUNKED"), chunks).await,
            vec![b'x'; 4096]
        );
    }
    #[tokio::test]
    async fn chunk_line_boundary_and_wire_budget_are_enforced_without_decoding_framing() {
        for length in [1023, 1024, 1025] {
            let wire = format!("1;{}\r\nx\r\n0\r\n", "a".repeat(length - 4)).into_bytes();
            let expected = if length <= 1024 {
                b"x".to_vec()
            } else {
                Vec::new()
            };
            assert_eq!(
                capture(&headers("transfer-encoding", "chunked"), wire).await,
                expected
            );
        }
        // Each complete chunk uses 1025 wire bytes; the next size line hits the
        // wire cap before another decoded byte can be admitted.
        let chunk = format!("1;{}\r\nx\r\n", "a".repeat(1018)).into_bytes();
        assert_eq!(chunk.len(), 1025);
        let wire = chunk.repeat(65);
        for limit in [65535, 65536, 65537] {
            let mut reader = BodyReader {
                stream: tokio::io::empty(),
                tail: wire[..limit].to_vec(),
                position: 0,
                processed: 0,
            };
            let mut prefix = Vec::new();
            let outcome = reader.capture(Framing::Chunked, &mut prefix).await;
            assert!(matches!(
                outcome,
                Err(CaptureError::Incomplete | CaptureError::Limit)
            ));
            assert_eq!(reader.processed, limit.min(65536));
            assert_eq!(prefix, vec![b'x'; 63]);
        }
    }
    #[tokio::test]
    async fn ambiguous_or_malformed_framing_discards_detail_but_partial_eof_keeps_decoded_progress()
    {
        let mut ambiguous = headers("content-length", "3");
        ambiguous.append(http::header::CONTENT_LENGTH, "3".parse().unwrap());
        assert!(capture(&ambiguous, b"raw".to_vec()).await.is_empty());
        let mut conflict = headers("content-length", "3");
        conflict.insert(http::header::TRANSFER_ENCODING, "chunked".parse().unwrap());
        assert!(capture(&conflict, b"raw".to_vec()).await.is_empty());
        let mut duplicate = headers("transfer-encoding", "chunked");
        duplicate.append(http::header::TRANSFER_ENCODING, "chunked".parse().unwrap());
        assert!(
            capture(&duplicate, b"3\r\nraw\r\n0\r\n".to_vec())
                .await
                .is_empty()
        );
        for value in ["", "-1", "+3", "3, 3", "three"] {
            assert!(
                capture(&headers("content-length", value), b"raw".to_vec())
                    .await
                    .is_empty()
            );
        }
        for value in ["gzip", "gzip, chunked", "chunked, chunked"] {
            assert!(
                capture(&headers("transfer-encoding", value), b"raw".to_vec())
                    .await
                    .is_empty()
            );
        }
        for wire in [
            b"3\r\nrawXX".as_slice(),
            b"3\r\nraw\r\nZ\r\n",
            b"1\nx\r\n",
            b"1\r\nx!",
            b"1;\0\r\nx\r\n0\r\n",
            b"1;name=\"unterminated\r\nx\r\n0\r\n",
        ] {
            assert!(
                capture(&headers("transfer-encoding", "chunked"), wire.to_vec())
                    .await
                    .is_empty()
            );
        }
        assert_eq!(
            capture(&headers("content-length", "9"), b"partial".to_vec()).await,
            b"partial"
        );
        assert_eq!(
            capture(&http::HeaderMap::new(), b"until eof".to_vec()).await,
            b"until eof"
        );
        assert_eq!(
            capture(
                &headers("transfer-encoding", "chunked"),
                b"3\r\nra".to_vec()
            )
            .await,
            b"ra"
        );
        assert_eq!(
            capture(
                &headers("transfer-encoding", "chunked"),
                b"1;name=\"quote\\\"value\";flag\r\nx\r\n0\r\n".to_vec()
            )
            .await,
            b"x"
        );
        // Declared lengths cannot drive allocation or arithmetic overflow.
        assert_eq!(
            capture(
                &headers("content-length", &"9".repeat(100)),
                vec![b'x'; 5000]
            )
            .await
            .len(),
            4096
        );
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use futures::{StreamExt, future::poll_fn};
    use std::{
        collections::VecDeque,
        io,
        pin::Pin,
        sync::{Arc, Mutex},
        task::{Context, Poll, Waker},
        time::Duration,
    };
    use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
    use tokio_tungstenite::tungstenite::{Error as WsError, Message, client::IntoClientRequest};

    #[derive(Default)]
    struct Trace {
        body_requested: bool,
        body_open: bool,
        body_waker: Option<Waker>,
        signal_waker: Option<Waker>,
        body_bytes: usize,
        eof_polls: usize,
        stall_at_end: bool,
        writes: Vec<u8>,
        drops: usize,
    }

    struct ScriptedIo {
        first_read: Vec<u8>,
        first_offset: usize,
        fragments: VecDeque<Vec<u8>>,
        fragment_offset: usize,
        trace: Arc<Mutex<Trace>>,
    }

    impl ScriptedIo {
        fn new(first_read: Vec<u8>, fragments: Vec<Vec<u8>>) -> (Self, Arc<Mutex<Trace>>) {
            let trace = Arc::new(Mutex::new(Trace::default()));
            (
                Self {
                    first_read,
                    first_offset: 0,
                    fragments: fragments
                        .into_iter()
                        .filter(|part| !part.is_empty())
                        .collect(),
                    fragment_offset: 0,
                    trace: trace.clone(),
                },
                trace,
            )
        }
    }

    impl AsyncRead for ScriptedIo {
        fn poll_read(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            out: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            let this = self.get_mut();
            if out.remaining() == 0 {
                return Poll::Ready(Ok(()));
            }
            if this.first_offset < this.first_read.len() {
                let count = out
                    .remaining()
                    .min(this.first_read.len() - this.first_offset);
                out.put_slice(&this.first_read[this.first_offset..this.first_offset + count]);
                this.first_offset += count;
                return Poll::Ready(Ok(()));
            }
            let (open, signal) = {
                let mut trace = this.trace.lock().expect("trace lock");
                trace.body_requested = true;
                if !trace.body_open {
                    trace.body_waker = Some(cx.waker().clone());
                }
                (trace.body_open, trace.signal_waker.take())
            };
            if let Some(signal) = signal {
                signal.wake();
            }
            if !open {
                return Poll::Pending;
            }
            let Some(fragment) = this.fragments.front() else {
                let mut trace = this.trace.lock().expect("trace lock");
                if trace.stall_at_end {
                    trace.body_waker = Some(cx.waker().clone());
                    return Poll::Pending;
                }
                trace.eof_polls += 1;
                return Poll::Ready(Ok(()));
            };
            let count = out.remaining().min(fragment.len() - this.fragment_offset);
            out.put_slice(&fragment[this.fragment_offset..this.fragment_offset + count]);
            this.fragment_offset += count;
            if this.fragment_offset == fragment.len() {
                this.fragments.pop_front();
                this.fragment_offset = 0;
            }
            this.trace.lock().expect("trace lock").body_bytes += count;
            Poll::Ready(Ok(()))
        }
    }

    impl AsyncWrite for ScriptedIo {
        fn poll_write(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            bytes: &[u8],
        ) -> Poll<io::Result<usize>> {
            self.trace
                .lock()
                .expect("trace lock")
                .writes
                .extend_from_slice(bytes);
            Poll::Ready(Ok(bytes.len()))
        }

        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    impl Drop for ScriptedIo {
        fn drop(&mut self) {
            self.trace.lock().expect("trace lock").drops += 1;
        }
    }

    fn request() -> tokio_tungstenite::tungstenite::handshake::client::Request {
        let mut request = "ws://rpc.example/private"
            .into_client_request()
            .expect("request");
        // RFC 6455 example key: fixes the independent successful response below.
        request.headers_mut().insert(
            "sec-websocket-key",
            http::HeaderValue::from_static("dGhlIHNhbXBsZSBub25jZQ=="),
        );
        request
    }

    async fn body_requested(trace: &Arc<Mutex<Trace>>) {
        poll_fn(|cx| {
            let mut trace = trace.lock().expect("trace lock");
            if trace.body_requested {
                Poll::Ready(())
            } else {
                trace.signal_waker = Some(cx.waker().clone());
                Poll::Pending
            }
        })
        .await;
    }

    fn release_body(trace: &Arc<Mutex<Trace>>) {
        let wake = {
            let mut trace = trace.lock().expect("trace lock");
            trace.body_open = true;
            trace.body_waker.take()
        };
        if let Some(wake) = wake {
            wake.wake();
        }
    }

    #[tokio::test]
    async fn delayed_fixed_length_uses_production_upgrade_and_retains_metadata() {
        for status in [403, 429, 502] {
            let body = "{\"error\":\"busy é\"}".as_bytes();
            let split = body
                .iter()
                .position(|byte| *byte == 0xc3)
                .expect("UTF-8 lead");
            let head = format!(
                "HTTP/1.1 {status} Refused\r\nContent-Length: {}\r\nX-RateLimit-Policy: first\r\nX-RateLimit-Policy: second\r\nRetry-After: 60\r\n\r\n",
                body.len()
            );
            let (io, trace) = ScriptedIo::new(
                head.into_bytes(),
                vec![
                    body[..split + 1].to_vec(),
                    body[split + 1..split + 2].to_vec(),
                    body[split + 2..].to_vec(),
                ],
            );
            let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
            let mut upgrading =
                Box::pin(upgrade(request(), io, WebSocketConfig::default(), deadline));
            tokio::select! {
                biased;
                _ = upgrading.as_mut() => panic!("upgrade finished before the delayed body was released"),
                _ = body_requested(&trace) => {}
            }
            assert_eq!(trace.lock().expect("trace lock").body_bytes, 0);
            release_body(&trace);
            let response = match upgrading.await {
                Err(UpgradeError::WebSocket(WsError::Http(response))) => response,
                _ => panic!("expected original HTTP refusal"),
            };
            assert_eq!(response.status().as_u16(), status);
            assert_eq!(response.body().as_deref(), Some(body));
            assert_eq!(
                response
                    .headers()
                    .get_all("x-ratelimit-policy")
                    .iter()
                    .count(),
                2
            );
            assert_eq!(
                crate::policy::response_detail(response.body().as_deref().expect("body")),
                "busy é"
            );
            let refusal = crate::policy::classify_handshake(
                status,
                response.headers(),
                response.body().as_deref(),
            );
            if status == 502 {
                assert!(refusal.is_none());
            } else {
                let refusal = refusal.expect("policy status remains terminal");
                assert_eq!(refusal.status_code, Some(status));
                assert_eq!(refusal.policy.as_deref(), Some("first"));
                assert_eq!(refusal.retry_after.as_deref(), Some("60"));
            }
            let observed = trace.lock().expect("trace lock");
            assert_eq!(observed.body_bytes, body.len());
            assert_eq!(observed.eof_polls, 0, "fixed length must not wait for EOF");
            assert_eq!(observed.drops, 1);
            assert!(observed.writes.starts_with(b"GET /private HTTP/1.1\r\n"));
            assert_eq!(
                observed
                    .writes
                    .windows(4)
                    .filter(|part| *part == b"GET ")
                    .count(),
                1
            );
        }
    }

    #[tokio::test]
    async fn successful_upgrade_preserves_two_frames_in_the_http_head_read() {
        let mut head_and_frames = b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n\r\n".to_vec();
        head_and_frames.extend_from_slice(b"\x81\x05first\x81\x06second");
        let (io, trace) = ScriptedIo::new(head_and_frames, vec![]);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        let mut socket = match upgrade(request(), io, WebSocketConfig::default(), deadline).await {
            Ok(socket) => socket,
            Err(_) => panic!("RFC example upgrade must succeed"),
        };
        assert!(matches!(socket.get_ref(), RetainedIo::Owned(_)));
        for expected in ["first", "second"] {
            let Poll::Ready(Some(Ok(Message::Text(value)))) = futures::poll!(socket.next()) else {
                panic!("buffered frame must be ready without another stream read");
            };
            assert_eq!(value.as_str(), expected);
        }
        assert!(!trace.lock().expect("trace lock").body_requested);
        drop(socket);
        assert_eq!(trace.lock().expect("trace lock").drops, 1);
    }
    #[tokio::test(start_paused = true)]
    async fn optional_body_uses_the_original_deadline_and_keeps_known_policy_progress() {
        let deadline = Instant::now() + Duration::from_secs(10);
        tokio::time::advance(Duration::from_secs(8)).await;
        let (io, trace) = ScriptedIo::new(
            b"HTTP/1.1 429 Refused\r\nContent-Length: 100\r\nX-RateLimit-Policy: public\r\nRetry-After: 9\r\n\r\n".to_vec(),
            vec![b"known detail".to_vec()],
        );
        trace.lock().expect("trace lock").stall_at_end = true;
        let mut upgrading = Box::pin(upgrade(request(), io, WebSocketConfig::default(), deadline));
        assert!(futures::poll!(upgrading.as_mut()).is_pending());
        assert!(trace.lock().expect("trace lock").body_requested);
        release_body(&trace);
        assert!(futures::poll!(upgrading.as_mut()).is_pending());
        assert_eq!(trace.lock().expect("trace lock").body_bytes, 12);
        tokio::time::advance(Duration::from_millis(1999)).await;
        assert!(futures::poll!(upgrading.as_mut()).is_pending());
        tokio::time::advance(Duration::from_millis(2)).await;
        let response = match upgrading.await {
            Err(UpgradeError::WebSocket(WsError::Http(response))) => response,
            _ => panic!("known HTTP policy must survive optional body timeout"),
        };
        let refusal =
            crate::policy::classify_handshake(429, response.headers(), response.body().as_deref())
                .expect("known policy");
        assert_eq!(response.body().as_deref(), Some(b"known detail".as_slice()));
        assert_eq!(refusal.policy.as_deref(), Some("public"));
        assert_eq!(refusal.retry_after.as_deref(), Some("9"));
        assert!(refusal.message.contains("known detail"));
        assert_eq!(trace.lock().expect("trace lock").drops, 1);
    }

    #[tokio::test(start_paused = true)]
    async fn malformed_chunk_bytes_reject_detail_before_waiting_for_more_input() {
        for wire in [b"1\r\nx!".as_slice(), b"1\r\nx\r\n1;\0"] {
            let (io, trace) = ScriptedIo::new(
                b"HTTP/1.1 429 Refused\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec(),
                vec![wire.to_vec()],
            );
            trace.lock().expect("trace lock").stall_at_end = true;
            release_body(&trace);
            let mut upgrading = Box::pin(upgrade(
                request(),
                io,
                WebSocketConfig::default(),
                Instant::now() + Duration::from_secs(10),
            ));
            let Poll::Ready(Err(UpgradeError::WebSocket(WsError::Http(response)))) =
                futures::poll!(upgrading.as_mut())
            else {
                panic!("known malformed bytes must reject detail before a stalled read");
            };
            assert_eq!(response.status().as_u16(), 429);
            assert_eq!(response.body().as_deref(), Some(b"".as_slice()));
            assert_eq!(trace.lock().expect("trace lock").drops, 1);
        }

        // A lone correct CR is incomplete, so retain trusted data at expiry.
        let (io, trace) = ScriptedIo::new(
            b"HTTP/1.1 429 Refused\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec(),
            vec![b"1\r\nx\r".to_vec()],
        );
        trace.lock().expect("trace lock").stall_at_end = true;
        release_body(&trace);
        let mut upgrading = Box::pin(upgrade(
            request(),
            io,
            WebSocketConfig::default(),
            Instant::now() + Duration::from_secs(1),
        ));
        assert!(futures::poll!(upgrading.as_mut()).is_pending());
        tokio::time::advance(Duration::from_millis(1001)).await;
        let response = match upgrading.await {
            Err(UpgradeError::WebSocket(WsError::Http(response))) => response,
            _ => panic!("known HTTP refusal must survive incomplete framing"),
        };
        assert_eq!(response.body().as_deref(), Some(b"x".as_slice()));
        assert_eq!(trace.lock().expect("trace lock").drops, 1);
    }

    #[tokio::test(start_paused = true)]
    async fn pre_header_timeout_and_body_cancellation_release_the_transport_once() {
        let (io, trace) = ScriptedIo::new(vec![], vec![]);
        let deadline = Instant::now() + Duration::from_secs(1);
        let mut upgrading = Box::pin(upgrade(request(), io, WebSocketConfig::default(), deadline));
        assert!(futures::poll!(upgrading.as_mut()).is_pending());
        tokio::time::advance(Duration::from_millis(1001)).await;
        assert!(matches!(upgrading.await, Err(UpgradeError::Timeout)));
        assert_eq!(trace.lock().expect("trace lock").drops, 1);

        let (io, trace) = ScriptedIo::new(
            b"HTTP/1.1 403 Refused\r\nContent-Length: 2\r\n\r\n".to_vec(),
            vec![b"no".to_vec()],
        );
        let mut upgrading = Box::pin(upgrade(
            request(),
            io,
            WebSocketConfig::default(),
            Instant::now() + Duration::from_secs(1),
        ));
        assert!(futures::poll!(upgrading.as_mut()).is_pending());
        assert!(trace.lock().expect("trace lock").body_requested);
        drop(upgrading);
        assert_eq!(trace.lock().expect("trace lock").drops, 1);
    }
}
