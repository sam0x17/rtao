//! Lifetime custody for a single exact author request, including captured writes.
use super::*;
use std::any::Any;

/// Single-use ownership transferred to a native author request.
///
/// This retains caller-supplied strong ownership through queued/captured writes;
/// it does not grant chain authority, authorize retries, or prove execution.
/// The SDK journal must perform its own durable authorization before constructing
/// this value. Internal shared lifetime references are not reusable send permits.
pub struct AuthorOwnership {
    hash: H256,
    custody: Arc<Custody>,
}

/// Local retirement acknowledgment, independent of pool acceptance or chain fate.
/// Cancellation of the caller's request does not by itself retire its writer.
pub struct AuthorRetirement(oneshot::Receiver<()>);

pub(super) struct Custody {
    _owner: Option<Box<dyn Any + Send + Sync>>,
    retired: Option<oneshot::Sender<()>>,
}
impl Drop for Custody {
    fn drop(&mut self) {
        drop(self._owner.take());
        if let Some(retired) = self.retired.take() {
            let _ = retired.send(());
        }
    }
}
impl AuthorOwnership {
    /// Bind a strong ownership guard to the hash of exactly one signed request.
    /// Returns an acknowledgment that fires only after every native queue/frame
    /// reference to that ownership has retired. It says nothing about execution.
    pub fn new(hash: H256, owner: impl Any + Send + Sync) -> (Self, AuthorRetirement) {
        let (retired, receiver) = oneshot::channel();
        (
            Self {
                hash,
                custody: Arc::new(Custody {
                    _owner: Some(Box::new(owner)),
                    retired: Some(retired),
                }),
            },
            AuthorRetirement(receiver),
        )
    }
    pub(super) fn for_bytes(self, bytes: &[u8]) -> Result<Arc<Custody>, SubmitError> {
        if self.hash != H256::from_slice(&Blake2b::<U32>::digest(bytes)) {
            return Err(SubmitError::Rpc(RpcError::Protocol(
                "author ownership hash mismatch".into(),
            )));
        }
        Ok(self.custody)
    }
}
impl AuthorRetirement {
    /// Await local writer retirement. This does not establish absence, inclusion
    /// or safe replacement: durable uncertain attempts must still be reconciled.
    pub async fn wait(self) -> Result<(), RpcError> {
        self.0.await.map_err(|_| RpcError::Closed)
    }
}

/// Field drop order is intentional: buffered I/O must be destroyed BEFORE any
/// retained ownership. This also holds if the entire supervisor is cancelled.
pub(super) struct OwnedSocket<S> {
    pub(super) io: S,
    pub(super) writers: Vec<Arc<Custody>>,
}
impl<S> OwnedSocket<S> {
    pub(super) fn new(io: S) -> Self {
        Self {
            io,
            writers: Vec::new(),
        }
    }
}

// Keep ownership while closing may flush an already captured frame. Timeout
// retires the actual socket (io field first), not a timer lease on its writers.
pub(super) async fn retire_socket<S>(mut socket: OwnedSocket<S>)
where
    S: Sink<Message, Error = WsError> + Unpin,
{
    let _ = timeout(Duration::from_millis(500), socket.io.close()).await;
}

#[cfg(test)]
mod tests;
