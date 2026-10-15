//! A lazy private Tokio thread for the facade's blocking adapters.
//!
//! Only owned futures cross the thread boundary. A facade can clone its async
//! client or snapshot into a future and delegate the actual operation without
//! running a second implementation of it.

use std::{
    any::Any,
    collections::BTreeMap,
    fmt,
    future::Future,
    panic::{AssertUnwindSafe, catch_unwind},
    pin::Pin,
    sync::{Arc, Mutex, Weak, mpsc},
    thread::{self, JoinHandle, ThreadId},
    time::{Duration, Instant},
};

use futures::FutureExt;
use tokio::sync::mpsc as task_channel;
use tokio_util::sync::CancellationToken;

const SHUTDOWN_WAIT: Duration = Duration::from_secs(5);
const BLOCKING_TASK_WAIT: Duration = Duration::from_secs(1);

type Task = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

/// Failure to start, use, or stop a private blocking runtime.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum BlockingError {
    /// The runtime has been closed and cannot be restarted.
    Closed,
    /// A blocking operation was attempted from this runtime's own thread.
    OwnThread,
    /// The operating system could not start the worker thread.
    ThreadStart(String),
    /// Tokio could not initialize the worker's runtime.
    RuntimeStart(String),
    /// A submitted future panicked while being polled.
    TaskPanicked(String),
    /// Shutdown or worker termination cancelled an outstanding future.
    TaskCancelled,
    /// The worker terminated before accepting a submitted future.
    WorkerStopped,
    /// The worker terminated because of an internal panic.
    WorkerPanicked,
    /// The worker did not finish within the bounded shutdown interval.
    ShutdownTimeout,
    /// A previous panic poisoned an internal synchronization lock.
    StatePoisoned,
    /// The runtime exhausted its task identifier space.
    TaskLimit,
}

impl fmt::Display for BlockingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Closed => formatter.write_str("blocking runtime is closed"),
            Self::OwnThread => formatter
                .write_str("cannot block on the blocking runtime from its own worker thread"),
            Self::ThreadStart(detail) => {
                write!(formatter, "cannot start blocking runtime thread: {detail}")
            }
            Self::RuntimeStart(detail) => {
                write!(formatter, "cannot initialize blocking runtime: {detail}")
            }
            Self::TaskPanicked(detail) => {
                write!(formatter, "blocking runtime task panicked: {detail}")
            }
            Self::TaskCancelled => formatter.write_str("blocking runtime task was cancelled"),
            Self::WorkerStopped => formatter.write_str("blocking runtime worker has stopped"),
            Self::WorkerPanicked => formatter.write_str("blocking runtime worker panicked"),
            Self::ShutdownTimeout => {
                formatter.write_str("blocking runtime worker did not stop within five seconds")
            }
            Self::StatePoisoned => {
                formatter.write_str("blocking runtime synchronization lock was poisoned")
            }
            Self::TaskLimit => {
                formatter.write_str("blocking runtime task identifier space exhausted")
            }
        }
    }
}

impl std::error::Error for BlockingError {}

/// A lazily started current-thread Tokio runtime on a dedicated OS thread.
///
/// Construction performs no I/O and starts no thread. Clones share one worker;
/// closing any clone permanently closes them all. The last clone also shuts
/// the worker down. A call from the worker itself is rejected before waiting,
/// so reentrant facade calls cannot deadlock the worker.
#[derive(Clone, Default)]
pub struct BlockingRuntime {
    inner: Arc<Inner>,
}

#[derive(Default)]
struct Inner {
    state: Mutex<State>,
    closing: Mutex<()>,
}

#[derive(Default)]
struct State {
    started: bool,
    closed: bool,
    worker_id: Option<ThreadId>,
    worker: Option<Worker>,
    close_result: Option<Result<(), BlockingError>>,
    next_task: u64,
    waiters: BTreeMap<u64, Box<dyn FnOnce() + Send>>,
}

struct Worker {
    sender: task_channel::UnboundedSender<Task>,
    cancel: CancellationToken,
    thread: JoinHandle<()>,
    stopped: mpsc::Receiver<bool>,
}

struct Completion<T> {
    sender: mpsc::SyncSender<Result<T, BlockingError>>,
    inner: Weak<Inner>,
    id: u64,
    completed: bool,
}

impl<T> Completion<T> {
    fn finish(mut self, result: Result<T, BlockingError>) {
        // Close may already have supplied cancellation. Neither competitor
        // waits for the caller to consume the single winning result.
        self.completed = true;
        let _ = self.sender.try_send(result);
    }
}

impl<T> Drop for Completion<T> {
    fn drop(&mut self) {
        if !self.completed {
            let _ = self.sender.try_send(Err(BlockingError::TaskCancelled));
        }
        if let Some(inner) = self.inner.upgrade() {
            let mut state = match inner.state.lock() {
                Ok(state) => state,
                Err(poisoned) => poisoned.into_inner(),
            };
            state.waiters.remove(&self.id);
        }
    }
}

impl fmt::Debug for BlockingRuntime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BlockingRuntime")
            .field("started", &self.is_started())
            .field("closed", &self.is_closed())
            .finish_non_exhaustive()
    }
}

impl BlockingRuntime {
    /// Construct a runtime without starting its thread or reactor.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the shared worker has successfully started at least once.
    pub fn is_started(&self) -> bool {
        self.inner
            .state
            .lock()
            .map(|state| state.started)
            .unwrap_or(false)
    }

    /// Whether this runtime has been permanently closed.
    pub fn is_closed(&self) -> bool {
        self.inner
            .state
            .lock()
            .map(|state| state.closed)
            .unwrap_or(true)
    }

    /// Whether the caller is this runtime's worker, without starting it.
    ///
    /// Facades can reject reentrant teardown before changing their own state.
    pub fn owns_current_thread(&self) -> bool {
        self.inner
            .state
            .lock()
            .map(|state| state.worker_id == Some(thread::current().id()))
            .unwrap_or(false)
    }

    /// Run an owned future on the private worker and wait for its result.
    ///
    /// The future and result must be `Send + 'static`; borrowed application
    /// state must first be placed in owned or shared values. Application errors
    /// remain the future's output, so a future returning `Result<T, E>` produces
    /// `Result<Result<T, E>, BlockingError>`. Panics are returned separately and
    /// leave the worker available for subsequent calls.
    pub fn run<F, T>(&self, future: F) -> Result<T, BlockingError>
    where
        F: Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        let (result_sender, result_receiver) = mpsc::sync_channel(1);
        let (sender, id) = {
            let mut state = self
                .inner
                .state
                .lock()
                .map_err(|_| BlockingError::StatePoisoned)?;
            if state.closed {
                return Err(BlockingError::Closed);
            }
            if state.worker_id == Some(thread::current().id()) {
                return Err(BlockingError::OwnThread);
            }
            if state.worker.is_none() {
                let worker = Worker::start()?;
                state.worker_id = Some(worker.thread.thread().id());
                state.started = true;
                state.worker = Some(worker);
            }
            let sender = state
                .worker
                .as_ref()
                .ok_or(BlockingError::WorkerStopped)?
                .sender
                .clone();
            let id = state.next_task;
            state.next_task = id.checked_add(1).ok_or(BlockingError::TaskLimit)?;
            let cancellation = result_sender.clone();
            state.waiters.insert(
                id,
                Box::new(move || {
                    let _ = cancellation.try_send(Err(BlockingError::TaskCancelled));
                }),
            );
            (sender, id)
        };
        let completion = Completion {
            sender: result_sender,
            inner: Arc::downgrade(&self.inner),
            id,
            completed: false,
        };
        let task = Box::pin(async move {
            let result = AssertUnwindSafe(future)
                .catch_unwind()
                .await
                .map_err(|payload| BlockingError::TaskPanicked(panic_message(payload)));
            completion.finish(result);
        });
        sender
            .send(task)
            .map_err(|_| BlockingError::WorkerStopped)?;
        result_receiver
            .recv()
            .map_err(|_| BlockingError::TaskCancelled)?
    }

    /// Cancel outstanding tasks and permanently stop the shared worker.
    ///
    /// Concurrent closes wait for the same shutdown, and repeated closes return
    /// its recorded result. A cold runtime closes without creating a worker.
    /// Teardown waits at most five seconds; a future that monopolizes its poll
    /// can delay worker exit beyond that bound. Tokio cannot forcibly stop an
    /// already running `spawn_blocking` function.
    pub fn close(&self) -> Result<(), BlockingError> {
        {
            let state = self
                .inner
                .state
                .lock()
                .map_err(|_| BlockingError::StatePoisoned)?;
            if state.worker_id == Some(thread::current().id()) {
                return Err(BlockingError::OwnThread);
            }
        }
        let _closing = self
            .inner
            .closing
            .lock()
            .map_err(|_| BlockingError::StatePoisoned)?;
        let (worker, waiters) = {
            let mut state = self
                .inner
                .state
                .lock()
                .map_err(|_| BlockingError::StatePoisoned)?;
            if let Some(result) = &state.close_result {
                return result.clone();
            }
            state.closed = true;
            (state.worker.take(), std::mem::take(&mut state.waiters))
        };
        // Wake callers before joining the worker, including when a future is
        // stuck inside a poll and cannot yet observe Tokio cancellation.
        for cancel in waiters.into_values() {
            cancel();
        }
        let result = worker.map_or(Ok(()), Worker::stop);
        self.inner
            .state
            .lock()
            .map_err(|_| BlockingError::StatePoisoned)?
            .close_result = Some(result.clone());
        result
    }
}

impl Worker {
    fn start() -> Result<Self, BlockingError> {
        let (sender, receiver) = task_channel::unbounded_channel();
        let cancel = CancellationToken::new();
        let worker_cancel = cancel.clone();
        let (ready_sender, ready_receiver) = mpsc::sync_channel(1);
        let (stopped_sender, stopped) = mpsc::sync_channel(1);
        let thread = thread::Builder::new()
            .name("rtao-blocking".into())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        let _ =
                            ready_sender.send(Err(BlockingError::RuntimeStart(error.to_string())));
                        return;
                    }
                };
                if ready_sender.send(Ok(())).is_err() {
                    runtime.shutdown_timeout(BLOCKING_TASK_WAIT);
                    return;
                }
                let outcome = catch_unwind(AssertUnwindSafe(|| {
                    runtime.block_on(drive(receiver, worker_cancel));
                }));
                runtime.shutdown_timeout(BLOCKING_TASK_WAIT);
                let _ = stopped_sender.send(outcome.is_ok());
            })
            .map_err(|error| BlockingError::ThreadStart(error.to_string()))?;
        match ready_receiver.recv() {
            Ok(Ok(())) => Ok(Self {
                sender,
                cancel,
                thread,
                stopped,
            }),
            Ok(Err(error)) => {
                let _ = thread.join();
                Err(error)
            }
            Err(_) => {
                let _ = thread.join();
                Err(BlockingError::WorkerPanicked)
            }
        }
    }

    fn stop(self) -> Result<(), BlockingError> {
        let started = Instant::now();
        self.cancel.cancel();
        drop(self.sender);
        if self.thread.thread().id() == thread::current().id() {
            // Last-owner Drop can happen inside a task; cancellation still
            // happens, but this thread must never wait for or join itself.
            return Ok(());
        }
        let outcome = match self
            .stopped
            .recv_timeout(SHUTDOWN_WAIT.saturating_sub(started.elapsed()))
        {
            Ok(true) => Ok(()),
            Ok(false) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                Err(BlockingError::WorkerPanicked)
            }
            Err(mpsc::RecvTimeoutError::Timeout) => return Err(BlockingError::ShutdownTimeout),
        };
        // The worker sends its outcome before thread teardown. TLS destructors
        // can still block after that signal, so joining immediately would turn
        // the bounded wait into an unbounded close.
        while !self.thread.is_finished() {
            let remaining = SHUTDOWN_WAIT.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                return Err(BlockingError::ShutdownTimeout);
            }
            thread::park_timeout(remaining.min(Duration::from_millis(1)));
        }
        let remaining = SHUTDOWN_WAIT.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return Err(BlockingError::ShutdownTimeout);
        }
        // is_finished observes completion of Rust's thread-main packet, which
        // can precede native TLS destruction. Join on a reaper so even a TLS
        // destructor that never returns cannot hold the caller indefinitely.
        let (joined_sender, joined_receiver) = mpsc::sync_channel(1);
        thread::Builder::new()
            .name("rtao-blocking-reap".into())
            .spawn(move || {
                let result = self
                    .thread
                    .join()
                    .map_err(|_| BlockingError::WorkerPanicked);
                let _ = joined_sender.send(result);
            })
            .map_err(|error| BlockingError::ThreadStart(error.to_string()))?;
        match joined_receiver.recv_timeout(SHUTDOWN_WAIT.saturating_sub(started.elapsed())) {
            Ok(result) => result?,
            Err(mpsc::RecvTimeoutError::Disconnected) => return Err(BlockingError::WorkerPanicked),
            Err(mpsc::RecvTimeoutError::Timeout) => return Err(BlockingError::ShutdownTimeout),
        }
        outcome
    }
}

async fn drive(mut receiver: task_channel::UnboundedReceiver<Task>, cancel: CancellationToken) {
    loop {
        tokio::select! {
            biased;
            () = cancel.cancelled() => return,
            task = receiver.recv() => match task {
                Some(task) => { tokio::spawn(task); }
                None => return,
            },
        }
    }
}

fn panic_message(payload: Box<dyn Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else if let Some(message) = payload.downcast_ref::<&'static str>() {
        (*message).into()
    } else {
        "non-string panic payload".into()
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        let state = match self.state.get_mut() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        state.closed = true;
        for cancel in std::mem::take(&mut state.waiters).into_values() {
            cancel();
        }
        if let Some(worker) = state.worker.take() {
            let _ = worker.stop();
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use std::sync::Barrier;

    #[test]
    fn construction_and_cold_close_do_not_start_a_worker() {
        let runtime = BlockingRuntime::new();
        let clone = runtime.clone();
        assert!(!runtime.is_started());
        assert!(!runtime.is_closed());
        assert!(!runtime.owns_current_thread());
        clone.close().unwrap();
        assert!(runtime.is_closed());
        assert!(!runtime.is_started());
        assert_eq!(runtime.run(async { 7 }), Err(BlockingError::Closed));
        runtime.close().unwrap();
    }

    #[test]
    fn calls_reuse_one_private_thread_with_a_live_tokio_reactor() {
        let runtime = BlockingRuntime::new();
        let caller = thread::current().id();
        let first = runtime
            .run(async {
                tokio::time::sleep(Duration::from_millis(1)).await;
                thread::current().id()
            })
            .unwrap();
        let second = runtime
            .clone()
            .run(async { thread::current().id() })
            .unwrap();
        assert_ne!(first, caller);
        assert_eq!(first, second);
        assert!(runtime.is_started());
        runtime.close().unwrap();
        assert_eq!(runtime.run(async {}), Err(BlockingError::Closed));
    }

    #[test]
    fn task_panics_are_reported_without_stopping_later_calls() {
        let runtime = BlockingRuntime::new();
        let failure: Result<(), BlockingError> = runtime.run(async { panic!("fixture panic") });
        assert_eq!(
            failure,
            Err(BlockingError::TaskPanicked("fixture panic".into()))
        );
        assert_eq!(runtime.run(async { 17 }).unwrap(), 17);
        runtime.close().unwrap();
    }

    #[test]
    fn worker_thread_reentry_and_close_refuse_before_waiting() {
        let runtime = BlockingRuntime::new();
        let nested = runtime.clone();
        let (owns_thread, run, close) = runtime
            .run(async move {
                (
                    nested.owns_current_thread(),
                    nested.run(async { 7 }),
                    nested.close(),
                )
            })
            .unwrap();
        assert!(owns_thread);
        assert_eq!(run, Err(BlockingError::OwnThread));
        assert_eq!(close, Err(BlockingError::OwnThread));
        assert!(!runtime.is_closed());
        runtime.close().unwrap();
    }

    #[test]
    fn close_cancels_an_outstanding_call_instead_of_leaving_its_caller_waiting() {
        let runtime = BlockingRuntime::new();
        let caller = runtime.clone();
        let (started_sender, started_receiver) = mpsc::sync_channel(1);
        let (result_sender, result_receiver) = mpsc::sync_channel(1);
        let thread = thread::spawn(move || {
            let result = caller.run(async move {
                started_sender.send(()).unwrap();
                std::future::pending::<()>().await;
            });
            result_sender.send(result).unwrap();
        });
        started_receiver.recv_timeout(SHUTDOWN_WAIT).unwrap();
        runtime.close().unwrap();
        assert_eq!(
            result_receiver.recv_timeout(SHUTDOWN_WAIT).unwrap(),
            Err(BlockingError::TaskCancelled)
        );
        thread.join().unwrap();
        runtime.close().unwrap();
    }

    #[test]
    fn concurrent_first_calls_start_only_one_worker() {
        let runtime = BlockingRuntime::new();
        let barrier = Arc::new(Barrier::new(9));
        let callers = (0..8)
            .map(|_| {
                let runtime = runtime.clone();
                let barrier = barrier.clone();
                thread::spawn(move || {
                    barrier.wait();
                    runtime.run(async { thread::current().id() }).unwrap()
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();
        let worker = runtime.run(async { thread::current().id() }).unwrap();
        for caller in callers {
            assert_eq!(caller.join().unwrap(), worker);
        }
        runtime.close().unwrap();
    }

    #[test]
    fn close_wakes_a_caller_even_before_its_future_can_yield() {
        let runtime = BlockingRuntime::new();
        let caller = runtime.clone();
        let closer = runtime.clone();
        let (started_sender, started_receiver) = mpsc::sync_channel(1);
        let (release_sender, release_receiver) = mpsc::sync_channel(1);
        let (result_sender, result_receiver) = mpsc::sync_channel(1);
        let caller_thread = thread::spawn(move || {
            let result = caller.run(async move {
                started_sender.send(()).unwrap();
                // Deliberately block this fixture's poll. Close must notify
                // its caller independently of the reactor's ability to run.
                release_receiver.recv().unwrap();
                17
            });
            result_sender.send(result).unwrap();
        });
        started_receiver.recv_timeout(SHUTDOWN_WAIT).unwrap();
        let close_thread = thread::spawn(move || closer.close());
        let result = result_receiver.recv_timeout(SHUTDOWN_WAIT);
        release_sender.send(()).unwrap();
        caller_thread.join().unwrap();
        close_thread.join().unwrap().unwrap();
        assert_eq!(result.unwrap(), Err(BlockingError::TaskCancelled));
        assert!(runtime.is_closed());
    }

    #[test]
    fn last_owner_drop_cancels_tasks_without_an_explicit_close() {
        struct Notice(mpsc::SyncSender<()>);
        impl Drop for Notice {
            fn drop(&mut self) {
                let _ = self.0.send(());
            }
        }
        let runtime = BlockingRuntime::new();
        let (sender, receiver) = mpsc::sync_channel(1);
        let notice = Notice(sender);
        runtime
            .run(async move {
                tokio::spawn(async move {
                    let _notice = notice;
                    std::future::pending::<()>().await;
                });
            })
            .unwrap();
        drop(runtime);
        receiver.recv_timeout(SHUTDOWN_WAIT).unwrap();
    }

    #[test]
    fn shutdown_deadline_includes_thread_local_destructors() {
        use std::cell::RefCell;

        struct BlockOnDrop {
            entered: mpsc::SyncSender<()>,
            release: mpsc::Receiver<()>,
            finished: mpsc::SyncSender<()>,
        }
        impl Drop for BlockOnDrop {
            fn drop(&mut self) {
                let _ = self.entered.send(());
                let _ = self.release.recv();
                let _ = self.finished.send(());
            }
        }
        thread_local! {
            static TEARDOWN: RefCell<Option<BlockOnDrop>> = const { RefCell::new(None) };
        }

        let runtime = BlockingRuntime::new();
        let closer = runtime.clone();
        let (entered_sender, entered_receiver) = mpsc::sync_channel(1);
        let (release_sender, release_receiver) = mpsc::sync_channel(1);
        let (finished_sender, finished_receiver) = mpsc::sync_channel(1);
        runtime
            .run(async move {
                TEARDOWN.with(|slot| {
                    *slot.borrow_mut() = Some(BlockOnDrop {
                        entered: entered_sender,
                        release: release_receiver,
                        finished: finished_sender,
                    });
                });
            })
            .unwrap();
        let (closed_sender, closed_receiver) = mpsc::sync_channel(1);
        let close_thread = thread::spawn(move || {
            let result = closer.close();
            let _ = closed_sender.send(result);
        });
        let entered = entered_receiver.recv_timeout(SHUTDOWN_WAIT);
        let closed = closed_receiver.recv_timeout(SHUTDOWN_WAIT + Duration::from_secs(2));
        // Release teardown before asserting so even a regression leaves no
        // blocked worker behind in the hosted test process.
        let _ = release_sender.send(());
        let finished = finished_receiver.recv_timeout(SHUTDOWN_WAIT);
        close_thread.join().unwrap();
        entered.unwrap();
        finished.unwrap();
        assert_eq!(closed.unwrap(), Err(BlockingError::ShutdownTimeout));
        assert!(runtime.is_closed());
        assert_eq!(runtime.close(), Err(BlockingError::ShutdownTimeout));
    }
}
