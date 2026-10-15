//! Injected APDU channels for exercising the core's Ledger protocol without HID.
//!
//! Hand-authored scripts are protocol fixtures. They are not emulator recordings
//! and provide no evidence about app acceptance, firmware or screen rendering.
//! A future bounded hosted emulator run can supply the same line-pair format.

#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

use std::{
    collections::VecDeque,
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use bittensor_core::{CoreError, signers::ApduTransport};
use rtao_core::ledger::LedgerDevice;

const MAX_APDU: usize = 260;
const MAX_REPLY: usize = 65_538;
const MAX_TRANSCRIPT: usize = 1_048_576;
const MAX_EXCHANGES: usize = 4_096;

fn error(message: impl Into<String>) -> CoreError {
    CoreError::Device(message.into())
}

/// One exact serialized request and raw reply, including its status word.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Exchange {
    /// Serialized APDU bytes emitted by the core.
    pub request: Vec<u8>,
    /// Device data followed by its two-byte big-endian status word.
    pub reply: Vec<u8>,
}

#[derive(Default)]
struct ReplayState {
    exchanges: VecDeque<Exchange>,
    completed: usize,
    failure: Option<String>,
}

/// An exact request checker handed to the core's real `LedgerDevice`.
#[derive(Clone)]
pub struct ReplayTransport(Arc<Mutex<ReplayState>>);

impl ReplayTransport {
    /// Validate a bounded script before any request can consume it.
    pub fn new(exchanges: impl IntoIterator<Item = Exchange>) -> Result<Self, CoreError> {
        let mut state = ReplayState::default();
        let mut bytes = 0usize;
        for exchange in exchanges {
            if state.exchanges.len() >= MAX_EXCHANGES
                || exchange.request.len() > MAX_APDU
                || exchange.reply.len() > MAX_REPLY
            {
                return Err(error("APDU script exceeds its exchange bounds"));
            }
            bytes = bytes
                .checked_add(exchange.request.len())
                .and_then(|count| count.checked_add(exchange.reply.len()))
                .ok_or_else(|| error("APDU script size overflow"))?;
            if bytes > MAX_TRANSCRIPT {
                return Err(error("APDU script exceeds 1 MiB"));
            }
            state.exchanges.push_back(exchange);
        }
        Ok(Self(Arc::new(Mutex::new(state))))
    }

    /// Parse strict `=> request_hex` / `<= response_hex` line pairs.
    ///
    /// Empty lines are allowed between pairs. Comments, incomplete pairs,
    /// reversed directions and odd-length hexadecimal values are rejected.
    pub fn from_transcript(text: &str) -> Result<Self, CoreError> {
        if text.len() > MAX_TRANSCRIPT {
            return Err(error("APDU transcript exceeds 1 MiB"));
        }
        let mut request = None;
        let mut exchanges = Vec::new();
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            if let Some(value) = line.strip_prefix("=> ") {
                if request.is_some() {
                    return Err(error("APDU transcript has consecutive requests"));
                }
                if value.len() > MAX_APDU.saturating_mul(2) {
                    return Err(error("APDU transcript request exceeds 260 bytes"));
                }
                request = Some(hex::decode(value).map_err(|_| error("invalid APDU request hex"))?);
            } else if let Some(value) = line.strip_prefix("<= ") {
                let request = request
                    .take()
                    .ok_or_else(|| error("APDU transcript reply has no request"))?;
                if value.len() > MAX_REPLY.saturating_mul(2) {
                    return Err(error("APDU transcript reply exceeds 65538 bytes"));
                }
                let reply = hex::decode(value).map_err(|_| error("invalid APDU reply hex"))?;
                exchanges.push(Exchange { request, reply });
                if exchanges.len() > MAX_EXCHANGES {
                    return Err(error("APDU transcript has too many exchanges"));
                }
            } else {
                return Err(error("invalid APDU transcript direction"));
            }
        }
        if request.is_some() {
            return Err(error("APDU transcript is missing its final reply"));
        }
        Self::new(exchanges)
    }

    /// Construct the core device while retaining this handle for exhaustion checks.
    pub fn device(&self) -> LedgerDevice {
        LedgerDevice::with_transport(self.clone())
    }

    /// Fail if any request mismatched, any unexpected call occurred or work remains.
    pub fn assert_finished(&self) -> Result<(), CoreError> {
        let state = self
            .0
            .lock()
            .map_err(|_| error("APDU replay lock poisoned"))?;
        if let Some(failure) = &state.failure {
            return Err(error(failure.clone()));
        }
        if !state.exchanges.is_empty() {
            return Err(error(format!(
                "APDU replay has {} unconsumed exchanges",
                state.exchanges.len()
            )));
        }
        Ok(())
    }
}

impl ApduTransport for ReplayTransport {
    fn exchange(&self, apdu: &[u8]) -> Result<Vec<u8>, CoreError> {
        let mut state = self
            .0
            .lock()
            .map_err(|_| error("APDU replay lock poisoned"))?;
        if let Some(failure) = &state.failure {
            return Err(error(failure.clone()));
        }
        let matches = state
            .exchanges
            .front()
            .is_some_and(|exchange| exchange.request == apdu);
        if !matches {
            let failure = format!(
                "unexpected or mismatched APDU at exchange {}",
                state.completed
            );
            state.failure = Some(failure.clone());
            return Err(error(failure));
        }
        let exchange = state
            .exchanges
            .pop_front()
            .ok_or_else(|| error("APDU replay lost its expected request"))?;
        state.completed = state.completed.saturating_add(1);
        Ok(exchange.reply)
    }
}

/// A bounded loopback TCP channel using Speculos's APDU framing.
///
/// This only connects to an existing listener. It never starts an emulator,
/// container or local service. Each failed exchange permanently retires its
/// stream, so partial frames cannot be reused as another response.
pub struct TcpTransport {
    stream: Mutex<Option<TcpStream>>,
    timeout: Duration,
}

impl TcpTransport {
    /// Connect to a loopback listener with a nonzero timeout of at most two minutes.
    pub fn connect(address: SocketAddr, timeout: Duration) -> Result<Self, CoreError> {
        if !address.ip().is_loopback() || timeout.is_zero() || timeout > Duration::from_secs(120) {
            return Err(error("APDU TCP needs loopback and a timeout in (0, 120s]"));
        }
        let stream = TcpStream::connect_timeout(&address, timeout)
            .map_err(|failure| error(format!("APDU TCP connection: {failure}")))?;
        stream
            .set_nodelay(true)
            .map_err(|failure| error(format!("APDU TCP configuration: {failure}")))?;
        Ok(Self {
            stream: Mutex::new(Some(stream)),
            timeout,
        })
    }

    /// Hand this channel to the core protocol implementation.
    pub fn into_device(self) -> LedgerDevice {
        LedgerDevice::with_transport(self)
    }
}

fn remaining(deadline: Instant) -> std::io::Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|duration| !duration.is_zero())
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::TimedOut, "APDU deadline elapsed"))
}

fn write_all(stream: &mut TcpStream, mut bytes: &[u8], deadline: Instant) -> std::io::Result<()> {
    while !bytes.is_empty() {
        stream.set_write_timeout(Some(remaining(deadline)?))?;
        let count = match stream.write(bytes) {
            Err(failure) if failure.kind() == std::io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        if count == 0 {
            return Err(std::io::ErrorKind::WriteZero.into());
        }
        bytes = bytes.get(count..).ok_or(std::io::ErrorKind::InvalidData)?;
    }
    Ok(())
}

fn read_exact(
    stream: &mut TcpStream,
    mut bytes: &mut [u8],
    deadline: Instant,
) -> std::io::Result<()> {
    while !bytes.is_empty() {
        stream.set_read_timeout(Some(remaining(deadline)?))?;
        let count = match stream.read(bytes) {
            Err(failure) if failure.kind() == std::io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        if count == 0 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        bytes = bytes
            .get_mut(count..)
            .ok_or(std::io::ErrorKind::InvalidData)?;
    }
    Ok(())
}

impl ApduTransport for TcpTransport {
    fn exchange(&self, apdu: &[u8]) -> Result<Vec<u8>, CoreError> {
        if apdu.len() > MAX_APDU {
            return Err(error("APDU request exceeds 260 bytes"));
        }
        let mut guard = self
            .stream
            .lock()
            .map_err(|_| error("APDU TCP lock poisoned"))?;
        let stream = guard
            .as_mut()
            .ok_or_else(|| error("APDU TCP stream was retired"))?;
        let deadline = Instant::now()
            .checked_add(self.timeout)
            .ok_or_else(|| error("APDU TCP deadline overflow"))?;
        let mut transfer = || -> std::io::Result<Vec<u8>> {
            let size = u32::try_from(apdu.len()).map_err(|_| std::io::ErrorKind::InvalidInput)?;
            write_all(stream, &size.to_be_bytes(), deadline)?;
            write_all(stream, apdu, deadline)?;
            let mut size = [0; 4];
            read_exact(stream, &mut size, deadline)?;
            // Speculos's response length excludes the trailing status word.
            let size = usize::try_from(u32::from_be_bytes(size))
                .ok()
                .and_then(|size| size.checked_add(2))
                .filter(|size| *size <= MAX_REPLY)
                .ok_or(std::io::ErrorKind::InvalidData)?;
            let mut reply = vec![0; size];
            read_exact(stream, &mut reply, deadline)?;
            Ok(reply)
        };
        match transfer() {
            Ok(reply) => Ok(reply),
            Err(failure) => {
                guard.take();
                Err(error(format!("APDU TCP exchange: {failure}")))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn transcript_direction_exhaustion_and_failure_are_strict() {
        for text in ["<= 9000", "=> 00", "=> 00\n=> 01", "=> 0\n<= 9000", "junk"] {
            assert!(ReplayTransport::from_transcript(text).is_err());
        }
        let replay =
            ReplayTransport::from_transcript("=> f900000000\n<= 0000640000001e9000\n").unwrap();
        assert!(replay.assert_finished().is_err());
        assert_eq!(replay.device().app_version().unwrap(), (100, 0, 30));
        replay.assert_finished().unwrap();
        assert!(replay.device().app_version().is_err());
        assert!(replay.assert_finished().is_err());
        let mismatch = ReplayTransport::from_transcript("=> 00\n<= 9000\n").unwrap();
        assert!(mismatch.device().app_version().is_err());
        assert!(mismatch.exchange(&[0]).is_err());
    }

    #[test]
    fn parser_and_direct_scripts_share_allocation_bounds() {
        assert!(ReplayTransport::from_transcript(&" ".repeat(MAX_TRANSCRIPT + 1)).is_err());
        let excessive_request = format!("=> {}\n<= 9000\n", "00".repeat(MAX_APDU + 1));
        assert!(ReplayTransport::from_transcript(&excessive_request).is_err());
        let excessive_reply = format!("=> f900000000\n<= {}\n", "00".repeat(MAX_REPLY + 1));
        assert!(ReplayTransport::from_transcript(&excessive_reply).is_err());
        assert!(
            ReplayTransport::new([Exchange {
                request: vec![0; MAX_APDU + 1],
                reply: vec![]
            }])
            .is_err()
        );
        assert!(
            ReplayTransport::new([Exchange {
                request: vec![],
                reply: vec![0; MAX_REPLY + 1]
            }])
            .is_err()
        );
    }
}
