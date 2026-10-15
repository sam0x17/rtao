//! Drop the real session socket after a node acknowledges an author watch.
//! The node receives the signed bytes once; reads and reconnects use the same
//! transparent loopback proxy and production Session implementation.

use futures::{SinkExt, StreamExt};
use serde_json::Value;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use tokio::{
    net::TcpListener,
    task::{JoinHandle, JoinSet},
};
use tokio_tungstenite::{accept_async, connect_async};
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct State {
    armed: AtomicBool,
    drops: AtomicUsize,
    watches: Mutex<Vec<Vec<u8>>>,
}

pub struct WatchProxy {
    endpoint: String,
    state: Arc<State>,
    cancel: CancellationToken,
    task: Option<JoinHandle<()>>,
}
impl WatchProxy {
    pub async fn start(upstream: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let endpoint = format!("ws://{}", listener.local_addr()?);
        let upstream = upstream.to_owned();
        let state = Arc::new(State::default());
        let cancel = CancellationToken::new();
        let worker_state = state.clone();
        let worker_cancel = cancel.clone();
        let task = tokio::spawn(async move {
            let mut connections = JoinSet::new();
            loop {
                tokio::select! {
                    () = worker_cancel.cancelled() => break,
                    Some(result) = connections.join_next(), if !connections.is_empty() => {
                        if let Ok(Err(error)) = result { eprintln!("watch proxy: {error}"); }
                    }
                    accepted = listener.accept() => {
                        let Ok((stream, _)) = accepted else { break };
                        let upstream = upstream.clone();
                        let state = worker_state.clone();
                        connections.spawn(async move {
                            let downstream = accept_async(stream).await.map_err(|e| e.to_string())?;
                            let (upstream, _) = connect_async(upstream).await.map_err(|e| e.to_string())?;
                            let (mut down_send, mut down_recv) = downstream.split();
                            let (mut up_send, mut up_recv) = upstream.split();
                            let mut drop_after_ack = None;
                            loop {
                                tokio::select! {
                                    message = down_recv.next() => {
                                        let Some(message) = message else { break };
                                        let message = message.map_err(|e| e.to_string())?;
                                        if let Ok(text) = message.to_text()
                                            && let Ok(frame) = serde_json::from_str::<Value>(text)
                                            && frame["method"] == "author_submitAndWatchExtrinsic"
                                        {
                                            let encoded = frame["params"][0].as_str().ok_or("author bytes absent")?;
                                            let bytes = hex::decode(encoded.strip_prefix("0x").ok_or("author bytes not hex")?).map_err(|e| e.to_string())?;
                                            state.watches.lock().map_err(|_| "watch lock poisoned")?.push(bytes);
                                            if state.armed.swap(false, Ordering::SeqCst) {
                                                drop_after_ack = Some(frame["id"].clone());
                                            }
                                        }
                                        up_send.send(message).await.map_err(|e| e.to_string())?;
                                    }
                                    message = up_recv.next() => {
                                        let Some(message) = message else { break };
                                        let message = message.map_err(|e| e.to_string())?;
                                        if let Some(id) = &drop_after_ack
                                            && let Ok(text) = message.to_text()
                                            && let Ok(frame) = serde_json::from_str::<Value>(text)
                                            && frame.get("id") == Some(id)
                                        {
                                            if frame.get("error").is_some() { return Err(format!("author watch rejected before disconnect: {frame}")); }
                                            state.drops.fetch_add(1, Ordering::SeqCst);
                                            break;
                                        }
                                        down_send.send(message).await.map_err(|e| e.to_string())?;
                                    }
                                }
                            }
                            Ok::<_, String>(())
                        });
                    }
                }
            }
            connections.abort_all();
            while connections.join_next().await.is_some() {}
        });
        Ok(Self {
            endpoint,
            state,
            cancel,
            task: Some(task),
        })
    }
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }
    pub fn arm(&self) {
        self.state.armed.store(true, Ordering::SeqCst);
    }
    pub fn drops(&self) -> usize {
        self.state.drops.load(Ordering::SeqCst)
    }
    pub fn watches(&self) -> Vec<Vec<u8>> {
        self.state
            .watches
            .lock()
            .expect("watch lock poisoned")
            .clone()
    }
    pub async fn close(&mut self) {
        self.cancel.cancel();
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}
impl Drop for WatchProxy {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

#[tokio::test]
async fn accepted_watch_is_lost_once_and_the_production_session_recovers() -> super::TestResult {
    let mut node = rtao_testkit::FakeNode::start().await?;
    node.mutate_state(|state| {
        state.set_handler("author_submitAndWatchExtrinsic", |_, _| {
            rtao_testkit::Reply::subscription("accepted", vec![serde_json::json!("ready")], false)
        })
    });
    let mut proxy = WatchProxy::start(node.url()).await?;
    let session =
        rtao_rpc::Session::connect(rtao_rpc::SessionConfig::new(proxy.endpoint().parse()?)).await?;
    let result = async {
        proxy.arm();
        let lost = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            session.submit_and_watch(&[1, 2]),
        )
        .await?;
        assert!(
            matches!(&lost, Err(rtao_rpc::SubmitError::FateUnknown { .. })),
            "accepted watch did not report unknown fate: {:?}",
            lost.err()
        );
        assert_eq!(proxy.drops(), 1);
        assert_eq!(proxy.watches(), vec![vec![1, 2]]);
        let _: serde_json::Value = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            session.request(rtao_rpc::ReadMethod::SystemHealth, serde_json::json!([])),
        )
        .await??;
        assert_eq!(
            proxy.watches(),
            vec![vec![1, 2]],
            "Session resent the accepted author bytes"
        );
        let mut next = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            session.submit_and_watch(&[3, 4]),
        )
        .await??;
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(10), next.next())
                .await?
                .ok_or("recovered watch ended")?
                .is_ok()
        );
        assert_eq!(proxy.watches(), vec![vec![1, 2], vec![3, 4]]);
        assert_eq!(proxy.drops(), 1);
        Ok::<_, Box<dyn std::error::Error>>(())
    }
    .await;
    session.close().await;
    proxy.close().await;
    node.shutdown().await;
    result
}
