#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use super::*;
use std::{
    io,
    pin::Pin,
    sync::Mutex,
    task::{Context, Poll, Waker},
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio_tungstenite::{WebSocketStream, tungstenite::protocol::Role};

#[derive(Default)]
struct Trace {
    written: Vec<u8>,
    write_left: Option<usize>,
    flush_open: bool,
    flush_waiting: bool,
    write_waiting: bool,
    fail_write: bool,
    dropped: bool,
    flushed: bool,
    waker: Option<Waker>,
}
struct Io(Arc<Mutex<Trace>>);
impl Drop for Io {
    fn drop(&mut self) {
        self.0.lock().unwrap().dropped = true;
    }
}
impl AsyncRead for Io {
    fn poll_read(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        _: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Poll::Pending
    }
}
impl AsyncWrite for Io {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        let mut trace = self.0.lock().unwrap();
        if trace.fail_write {
            return Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()));
        }
        let count = trace.write_left.unwrap_or(bytes.len()).min(bytes.len());
        if count == 0 {
            trace.write_waiting = true;
            trace.waker = Some(cx.waker().clone());
            return Poll::Pending;
        }
        trace.written.extend_from_slice(&bytes[..count]);
        if let Some(left) = &mut trace.write_left {
            *left -= count;
        }
        Poll::Ready(Ok(count))
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let mut trace = self.0.lock().unwrap();
        if trace.fail_write {
            return Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()));
        }
        if trace.flush_open {
            trace.flushed = true;
            Poll::Ready(Ok(()))
        } else {
            trace.flush_waiting = true;
            trace.waker = Some(cx.waker().clone());
            Poll::Pending
        }
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.poll_flush(cx)
    }
}
struct Scope {
    held: Mutex<bool>,
    trace: Arc<Mutex<Trace>>,
}
struct Guard(Arc<Scope>);
impl Scope {
    fn claim(self: &Arc<Self>) -> Option<Guard> {
        let mut held = self.held.lock().unwrap();
        if *held {
            None
        } else {
            *held = true;
            Some(Guard(self.clone()))
        }
    }
}
impl Drop for Guard {
    fn drop(&mut self) {
        let trace = self.0.trace.lock().unwrap();
        // A guard may retire before capture or after a successful flush, but
        // never while an actual socket still owns an unflushed captured frame.
        assert!(
            trace.written.is_empty() || trace.flushed || trace.dropped,
            "scope released before buffered socket retired"
        );
        *self.0.held.lock().unwrap() = false;
    }
}
fn harness() -> (Session, Supervisor) {
    let config = SessionConfig::new(Url::parse("ws://127.0.0.1:1").unwrap());
    let tls = Arc::new(
        rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(rustls::RootCertStore::empty())
        .with_no_client_auth(),
    );
    let (commands, receiver) = mpsc::unbounded_channel();
    let (state, state_rx) = watch::channel(SessionState::Connected {
        url: config.pool.urls[0].clone(),
    });
    let (events, _) = broadcast::channel(8);
    let mut archive = config.clone();
    archive.pool.urls.clear();
    let session = Session {
        inner: Arc::new(Inner {
            commands,
            state: state_rx,
            events: events.clone(),
            ids: AtomicU64::new(1),
            archive: ArchivePool {
                config: archive,
                session: tokio::sync::OnceCell::new(),
                closing: watch::channel(false).0,
            },
        }),
    };
    let mut supervisor = Supervisor::new(config, tls, receiver, state, events);
    supervisor.epoch = 1;
    (session, supervisor)
}
async fn test_socket(trace: &Arc<Mutex<Trace>>) -> OwnedSocket<WebSocketStream<Io>> {
    OwnedSocket::new(WebSocketStream::from_raw_socket(Io(trace.clone()), Role::Client, None).await)
}
fn ownership(
    trace: &Arc<Mutex<Trace>>,
    bytes: &[u8],
) -> (
    AuthorOwnership,
    AuthorRetirement,
    Arc<Scope>,
    std::sync::Weak<Custody>,
) {
    let scope = Arc::new(Scope {
        held: Mutex::new(false),
        trace: trace.clone(),
    });
    let (owner, retired) = AuthorOwnership::new(
        H256::from_slice(&Blake2b::<U32>::digest(bytes)),
        scope.claim().unwrap(),
    );
    let weak = Arc::downgrade(&owner.custody);
    (owner, retired, scope, weak)
}
fn wake(trace: &Arc<Mutex<Trace>>, mutate: impl FnOnce(&mut Trace)) {
    let mut trace = trace.lock().unwrap();
    mutate(&mut trace);
    if let Some(waker) = trace.waker.take() {
        waker.wake();
    }
}
#[tokio::test]
async fn cancellation_before_queue_capture_retires_without_a_write() {
    let (session, mut supervisor) = harness();
    let bytes = [7u8; 8];
    let trace = Arc::new(Mutex::new(Trace::default()));
    let (owner, retired, scope, weak) = ownership(&trace, &bytes);
    let mut call = Box::pin(session.submit_owned(&bytes, owner));
    assert!(futures::poll!(call.as_mut()).is_pending());
    drop(call);
    while let Ok(command) = supervisor.commands.try_recv() {
        supervisor.command(Some(command));
    }
    assert_eq!(weak.strong_count(), 0);
    retired.wait().await.unwrap();
    assert!(scope.claim().is_some());
    let mut socket = test_socket(&trace).await;
    supervisor.release_queue(&mut socket).await.unwrap();
    assert!(trace.lock().unwrap().written.is_empty());
}
#[tokio::test]
async fn cancelled_caller_cannot_release_captured_real_websocket_flush_ownership() {
    let (session, mut supervisor) = harness();
    let bytes = [8u8; 16];
    let trace = Arc::new(Mutex::new(Trace::default()));
    let (owner, retired, scope, weak) = ownership(&trace, &bytes);
    let mut call = Box::pin(session.submit_owned(&bytes, owner));
    assert!(futures::poll!(call.as_mut()).is_pending());
    let command = supervisor.commands.recv().await;
    supervisor.command(command);
    let mut socket = test_socket(&trace).await;
    let mut writing = Box::pin(supervisor.release_queue(&mut socket));
    assert!(futures::poll!(writing.as_mut()).is_pending());
    assert!(trace.lock().unwrap().flush_waiting);
    assert_eq!(
        weak.strong_count(),
        2,
        "pending and captured writer both own custody"
    );
    drop(call);
    assert!(futures::poll!(writing.as_mut()).is_pending());
    assert_eq!(
        weak.strong_count(),
        1,
        "Cancel removed Pending, captured frame still owns custody"
    );
    assert!(scope.claim().is_none());
    let mut retired = Box::pin(retired.wait());
    assert!(futures::poll!(retired.as_mut()).is_pending());
    wake(&trace, |t| t.flush_open = true);
    writing.await.unwrap();
    retired.await.unwrap();
    assert_eq!(weak.strong_count(), 0);
    assert!(scope.claim().is_some());
}
#[tokio::test(start_paused = true)]
async fn partial_write_shutdown_ack_waits_for_actual_socket_retirement() {
    let (session, mut supervisor) = harness();
    let bytes = [9u8; 16];
    let trace = Arc::new(Mutex::new(Trace {
        write_left: Some(7),
        ..Default::default()
    }));
    let (owner, retired, scope, weak) = ownership(&trace, &bytes);
    let mut call = Box::pin(session.submit_owned(&bytes, owner));
    assert!(futures::poll!(call.as_mut()).is_pending());
    let command = supervisor.commands.recv().await;
    supervisor.command(command);
    let mut socket = test_socket(&trace).await;
    let mut writing = Box::pin(supervisor.release_queue(&mut socket));
    assert!(futures::poll!(writing.as_mut()).is_pending());
    assert!(trace.lock().unwrap().write_waiting);
    assert_eq!(trace.lock().unwrap().written.len(), 7);
    drop(call);
    assert!(futures::poll!(writing.as_mut()).is_pending());
    assert_eq!(weak.strong_count(), 1);
    let mut closing_session = Box::pin(session.close());
    assert!(futures::poll!(closing_session.as_mut()).is_pending());
    assert!(matches!(writing.await, Err(RpcError::Closed)));
    assert!(scope.claim().is_none());
    let mut closing_socket = Box::pin(retire_socket(socket));
    assert!(futures::poll!(closing_socket.as_mut()).is_pending());
    assert!(scope.claim().is_none());
    assert!(futures::poll!(closing_session.as_mut()).is_pending());
    tokio::time::advance(Duration::from_millis(500)).await;
    closing_socket.await;
    assert!(trace.lock().unwrap().dropped);
    retired.wait().await.unwrap();
    assert!(scope.claim().is_some());
    supervisor.finish(RpcError::Closed);
    closing_session.await;
}
#[tokio::test]
async fn write_error_and_connection_reset_never_replay_the_owned_author_frame() {
    for partial in [false, true] {
        let (session, mut supervisor) = harness();
        let bytes = [10u8; 16];
        let trace = Arc::new(Mutex::new(Trace {
            write_left: partial.then_some(7),
            fail_write: !partial,
            ..Default::default()
        }));
        let (owner, retired, scope, weak) = ownership(&trace, &bytes);
        let mut call = Box::pin(session.submit_owned(&bytes, owner));
        assert!(futures::poll!(call.as_mut()).is_pending());
        let command = supervisor.commands.recv().await;
        supervisor.command(command);
        let mut socket = test_socket(&trace).await;
        let mut writing = Box::pin(supervisor.release_queue(&mut socket));
        if partial {
            assert!(futures::poll!(writing.as_mut()).is_pending());
            assert_eq!(trace.lock().unwrap().written.len(), 7);
            wake(&trace, |t| t.fail_write = true);
        }
        assert!(writing.await.is_err());
        assert!(scope.claim().is_none());
        supervisor.connection_lost(RpcError::Transport("synthetic disconnect".into()));
        assert!(matches!(call.await, Err(SubmitError::FateUnknown { .. })));
        assert!(supervisor.pending.is_empty());
        assert_eq!(weak.strong_count(), 1);
        retire_socket(socket).await;
        assert!(trace.lock().unwrap().dropped);
        retired.wait().await.unwrap();
        assert!(scope.claim().is_some());
        let fresh_trace = Arc::new(Mutex::new(Trace {
            flush_open: true,
            ..Default::default()
        }));
        let mut fresh = test_socket(&fresh_trace).await;
        supervisor.resend(&mut fresh).await.unwrap();
        supervisor.release_queue(&mut fresh).await.unwrap();
        assert!(fresh_trace.lock().unwrap().written.is_empty());
    }
}
#[tokio::test]
async fn owned_author_hash_mismatch_never_enters_the_supervisor_queue() {
    let (session, mut supervisor) = harness();
    let trace = Arc::new(Mutex::new(Trace::default()));
    let (owner, retired, scope, _) = ownership(&trace, &[1]);
    assert!(matches!(
        session.submit_owned(&[2], owner).await,
        Err(SubmitError::Rpc(RpcError::Protocol(_)))
    ));
    retired.wait().await.unwrap();
    assert!(scope.claim().is_some());
    assert!(supervisor.commands.try_recv().is_err());
}

#[tokio::test]
async fn owned_watch_retires_after_flush_and_ack_and_keeps_native_status_routing() {
    let (session, mut supervisor) = harness();
    let bytes = [11u8; 16];
    let trace = Arc::new(Mutex::new(Trace {
        flush_open: true,
        ..Default::default()
    }));
    let (owner, retired, scope, weak) = ownership(&trace, &bytes);
    let mut call = Box::pin(session.submit_and_watch_owned(&bytes, owner));
    assert!(futures::poll!(call.as_mut()).is_pending());
    let command = supervisor.commands.recv().await;
    supervisor.command(command);
    let id = *supervisor.pending.keys().next().unwrap();
    let mut socket = test_socket(&trace).await;
    supervisor.release_queue(&mut socket).await.unwrap();
    assert!(trace.lock().unwrap().flushed);
    assert!(scope.claim().is_none());
    assert_eq!(
        weak.strong_count(),
        1,
        "Pending retains custody until acknowledgment"
    );
    let mut retired = Box::pin(retired.wait());
    assert!(futures::poll!(retired.as_mut()).is_pending());
    assert!(
        supervisor
            .inbound(Message::Text(
                json!({"id":id,"result":"owned-watch"}).to_string().into()
            ))
            .is_ok()
    );
    let mut subscription = call.await.unwrap();
    retired.await.unwrap();
    assert_eq!(weak.strong_count(), 0);
    assert!(scope.claim().is_some());
    let finalized = H256::repeat_byte(12);
    assert!(supervisor.inbound(Message::Text(json!({"method":"author_extrinsicUpdate","params":{"subscription":"owned-watch","result":{"finalized":finalized}}}).to_string().into())).is_ok());
    assert!(
        matches!(subscription.next().await, Some(Ok(TxStatus::Finalized(hash))) if hash == finalized)
    );
    drop(subscription);
}
