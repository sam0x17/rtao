//! Offline probes that feed the same inbound path as a connected session.

use super::*;
use std::{
    sync::OnceLock,
    task::{Context, Poll, Waker},
};

struct Probe {
    supervisor: Supervisor,
    receivers: Vec<(u64, oneshot::Receiver<Response>)>,
}

#[derive(Debug, PartialEq)]
struct Observation {
    terminal: Option<RpcError>,
    replies: Vec<(u64, Option<Result<Value, RpcError>>)>,
    pending: Vec<u64>,
    subscriptions: Vec<(String, Vec<Result<Value, RpcError>>)>,
    early: Vec<(String, Vec<Value>)>,
    failures: u32,
}

impl Probe {
    fn new() -> Result<Self, RpcError> {
        // No native certificate loading, socket, runtime, or background task.
        static TLS: OnceLock<Result<Arc<rustls::ClientConfig>, RpcError>> = OnceLock::new();
        let tls = TLS
            .get_or_init(|| {
                rustls::ClientConfig::builder_with_provider(Arc::new(
                    rustls::crypto::ring::default_provider(),
                ))
                .with_safe_default_protocol_versions()
                .map(|builder| {
                    Arc::new(
                        builder
                            .with_root_certificates(rustls::RootCertStore::empty())
                            .with_no_client_auth(),
                    )
                })
                .map_err(|error| RpcError::Protocol(error.to_string()))
            })
            .clone()?;
        let mut config = SessionConfig::new(
            Url::parse("ws://127.0.0.1:1")
                .map_err(|error| RpcError::Protocol(error.to_string()))?,
        );
        config.subscription_buffer = 2;
        config.early_notification_ids = 2;
        let (_commands, command_rx) = mpsc::unbounded_channel();
        let (state, _) = watch::channel(SessionState::Reconnecting { attempt: 2 });
        let (events, _) = broadcast::channel(8);
        let mut supervisor = Supervisor::new(config, tls, command_rx, state, events);
        supervisor.epoch = 1;
        supervisor.failures = 2;
        let mut receivers = Vec::new();
        for id in 1..=4 {
            let (sender, receiver) = oneshot::channel();
            supervisor.pending.insert(
                id,
                Pending {
                    id,
                    group: id,
                    frame: Value::Null,
                    method: "fuzz_frame".into(),
                    sender,
                    sent: true,
                    subscription: (id == 2).then(|| SubscriptionRequest {
                        unsubscribe: "fuzz_unsubscribe".into(),
                    }),
                    hash: (id == 3).then(|| H256::repeat_byte(3)),
                    started: Instant::now(),
                    waited: Duration::ZERO,
                    required_epoch: None,
                    last_sent_epoch: 1,
                },
            );
            if id == 4 {
                drop(receiver);
            } else {
                receivers.push((id, receiver));
            }
        }
        for id in ["live", "7"] {
            supervisor.subscriptions.insert(
                id.into(),
                ActiveSubscription {
                    queue: Queue::new(2),
                    hash: None,
                    request_id: 5,
                    unsubscribe: "fuzz_unsubscribe".into(),
                },
            );
        }
        supervisor.closed_ids.push_back("closed".into());
        Ok(Self {
            supervisor,
            receivers,
        })
    }

    fn observe(mut self, terminal: Option<RpcError>) -> Observation {
        let replies = self
            .receivers
            .iter_mut()
            .map(|(id, receiver)| {
                let reply = match receiver.try_recv() {
                    Ok(reply) => Some(reply.map(|reply| reply.value).map_err(WireError::into_rpc)),
                    Err(oneshot::error::TryRecvError::Empty) => None,
                    Err(oneshot::error::TryRecvError::Closed) => Some(Err(RpcError::Closed)),
                };
                (*id, reply)
            })
            .collect();
        assert!(self.supervisor.early.len() <= self.supervisor.config.early_notification_ids);
        let early = self
            .supervisor
            .early
            .iter()
            .map(|(id, items)| {
                assert!(items.len() <= self.supervisor.config.subscription_buffer);
                (id.clone(), items.iter().cloned().collect())
            })
            .collect();
        let subscriptions = self
            .supervisor
            .subscriptions
            .iter()
            .map(|(id, active)| {
                let mut context = Context::from_waker(Waker::noop());
                let mut items = Vec::new();
                while let Poll::Ready(Some(item)) = active.queue.poll(&mut context) {
                    items.push(item.map_err(WireError::into_rpc));
                    assert!(items.len() <= self.supervisor.config.subscription_buffer);
                }
                (id.clone(), items)
            })
            .collect();
        let observation = Observation {
            terminal,
            replies,
            pending: self.supervisor.pending.keys().copied().collect(),
            subscriptions,
            early,
            failures: self.supervisor.failures,
        };
        self.supervisor.pending.clear();
        self.supervisor.update_gauge();
        observation
    }
}

/// Exercise the production inbound parser and dispatcher with bounded offline state.
///
/// The probe seeds ordinary, subscription, submit, and cancelled requests plus
/// live and closed subscriptions. Valid UTF-8 must produce identical observable
/// replies and retained notifications whether carried by binary or text frames.
/// Setup errors are returned; parser panics and violated bounds remain fuzz failures.
#[cfg(feature = "fuzzing")]
pub fn fuzz_json_rpc_frame(bytes: &[u8]) -> Result<(), RpcError> {
    let mut binary = Probe::new()?;
    let error = binary
        .supervisor
        .inbound(Message::Binary(bytes.to_vec().into()))
        .err();
    let binary = binary.observe(error);
    if let Ok(text) = std::str::from_utf8(bytes) {
        let mut probe = Probe::new()?;
        let error = probe
            .supervisor
            .inbound(Message::Text(text.to_owned().into()))
            .err();
        assert_eq!(binary, probe.observe(error));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "fuzzing")]
    #[test]
    fn binary_and_text_probe_agree_for_batch_errors_and_malformed_json() -> Result<(), RpcError> {
        for frame in [
            &b"\xff{"[..],
            &b"{broken"[..],
            &b"null"[..],
            &br#"[{"id":1,"result":42},{"id":2,"result":7},{"params":{"subscription":7,"result":true}}]"#[..],
            &br#"{"id":3,"error":{"code":-32005,"message":"quota","data":{"policy":"slow"}}}"#[..],
        ] {
            fuzz_json_rpc_frame(frame)?;
        }
        Ok(())
    }

    fn observe(value: Value) -> Result<Observation, RpcError> {
        let mut probe = Probe::new()?;
        let error = probe
            .supervisor
            .inbound(Message::Text(value.to_string().into()))
            .err();
        Ok(probe.observe(error))
    }

    #[test]
    fn batch_correlates_replies_and_drains_early_subscription_notifications() -> Result<(), RpcError>
    {
        let observed = observe(json!([
            {"params":{"subscription":"fresh","result":10}},
            {"id":2,"result":"fresh"},
            {"params":{"subscription":"fresh","result":20}},
            {"id":1,"result":{"ok":true}},
            {"id":1,"result":"duplicate"},
            {"id":4,"result":"cancelled"}
        ]))?;
        assert_eq!(observed.terminal, None);
        assert_eq!(
            observed.replies,
            vec![
                (1, Some(Ok(json!({"ok":true})))),
                (2, Some(Ok(json!("fresh")))),
                (3, None)
            ]
        );
        assert_eq!(observed.pending, vec![3]);
        assert_eq!(
            observed.subscriptions[1],
            ("fresh".into(), vec![Ok(json!(10)), Ok(json!(20))])
        );
        assert!(observed.early.is_empty());
        Ok(())
    }

    #[test]
    fn batch_bounds_live_and_early_queues_and_normalizes_numeric_ids() -> Result<(), RpcError> {
        let observed = observe(json!([
            {"params":{"subscription":"live","result":1}},
            {"params":{"subscription":"live","result":2}},
            {"params":{"subscription":"live","result":3}},
            {"params":{"subscription":7,"result":7}},
            {"params":{"subscription":"closed","result":"ignored"}},
            {"params":{"subscription":"a","result":1}},
            {"params":{"subscription":"b","result":1}},
            {"params":{"subscription":"b","result":2}},
            {"params":{"subscription":"b","result":3}},
            {"params":{"subscription":"c","result":1}}
        ]))?;
        assert_eq!(
            observed.subscriptions,
            vec![
                ("7".into(), vec![Ok(json!(7))]),
                ("live".into(), vec![Ok(json!(2)), Ok(json!(3))])
            ]
        );
        assert_eq!(
            observed.early,
            vec![
                ("b".into(), vec![json!(2), json!(3)]),
                ("c".into(), vec![json!(1)])
            ]
        );
        Ok(())
    }

    #[test]
    fn policy_refusal_stops_batch_after_delivering_correlated_error() -> Result<(), RpcError> {
        let observed = observe(json!([
            {"id":1,"error":{"code":-32005,"message":"rate limit exceeded","data":{"policy":"slow"}}},
            {"id":3,"result":"not reached"}
        ]))?;
        assert!(matches!(observed.terminal, Some(RpcError::Policy(_))));
        assert!(matches!(
            observed.replies[0].1,
            Some(Err(RpcError::Policy(_)))
        ));
        assert_eq!(observed.pending, vec![2, 3, 4]);
        Ok(())
    }

    #[test]
    fn submit_errors_remain_submit_errors_and_discarded_state_takes_precedence()
    -> Result<(), RpcError> {
        let mut probe = Probe::new()?;
        probe.supervisor.inbound(Message::Text(json!({
            "id":3,"error":{"code":-32005,"message":"rate limit exceeded","data":"State already discarded for 0x123"}
        }).to_string().into()))?;
        let response = probe.receivers[2].1.try_recv();
        assert!(
            matches!(response, Ok(Err(WireError::Submit(SubmitError::Rpc(RpcError::StateDiscarded { block })))) if block == "0x123")
        );
        probe.observe(None);
        Ok(())
    }

    #[test]
    fn malformed_frames_and_invalid_subscription_ids_do_not_create_state() -> Result<(), RpcError> {
        let mut probe = Probe::new()?;
        probe
            .supervisor
            .inbound(Message::Binary(vec![0xff, b'{'].into()))?;
        probe.supervisor.inbound(Message::Text("{broken".into()))?;
        probe.supervisor.inbound(Message::Text(
            json!([
                {"id":2,"result":[]},
                {"id":"1","result":"ignored"},
                {"params":{"subscription":null,"result":"ignored"}},
                7, null
            ])
            .to_string()
            .into(),
        ))?;
        let observed = probe.observe(None);
        assert!(matches!(
            observed.replies[1].1,
            Some(Err(RpcError::Protocol(_)))
        ));
        assert_eq!(observed.pending, vec![1, 3, 4]);
        assert!(observed.early.is_empty());
        assert_eq!(observed.failures, 0);
        Ok(())
    }

    #[test]
    fn discovery_preserves_retry_count_but_delivers_policy_refusal() -> Result<(), RpcError> {
        let observed = observe(json!({"id":0,"result":{"methods":[]}}))?;
        assert_eq!(observed.failures, 2);
        assert_eq!(observed.pending, vec![1, 2, 3, 4]);
        let observed = observe(json!({"id":0,"error":{"code":-32004,"data":{"policy":"denied"}}}))?;
        assert_eq!(observed.failures, 2);
        assert!(matches!(observed.terminal, Some(RpcError::Policy(_))));
        Ok(())
    }
}
