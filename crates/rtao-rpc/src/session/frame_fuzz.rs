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
                    custody: None,
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
                    queue: Queue::new(2, supervisor.subscription_budget.clone()),
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
        assert!(self.supervisor.early_bytes <= self.supervisor.config.early_notification_bytes);
        assert!(
            self.supervisor.subscription_budget.retained_bytes()
                <= self.supervisor.config.subscription_buffer_bytes
        );
        let early = self
            .supervisor
            .early
            .iter()
            .map(|(id, items)| {
                assert!(items.len() <= self.supervisor.config.subscription_buffer);
                (
                    id.clone(),
                    items.iter().map(|item| item.value.clone()).collect(),
                )
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
    fn notification(probe: &mut Probe, id: &str, value: Value) -> Result<(), RpcError> {
        probe.supervisor.inbound(Message::Text(
            json!({
                "params": {"subscription": id, "result": value}
            })
            .to_string()
            .into(),
        ))
    }

    #[test]
    fn early_byte_budget_evicts_oldest_globally_and_accounts_for_identity_bytes()
    -> Result<(), RpcError> {
        let mut probe = Probe::new()?;
        let payload = json!("x".repeat(128));
        let charge = notification_bytes(&payload) + Supervisor::early_identity_bytes("a");
        probe.supervisor.config.early_notification_bytes = 2 * charge;
        notification(&mut probe, "a", payload.clone())?;
        notification(&mut probe, "b", payload.clone())?;
        assert_eq!(probe.supervisor.early_bytes, 2 * charge);
        notification(&mut probe, "a", payload.clone())?;
        assert_eq!(probe.supervisor.early_bytes, 2 * charge);
        assert_eq!(
            probe
                .supervisor
                .early
                .iter()
                .map(|(id, items)| (id.as_str(), items.len()))
                .collect::<Vec<_>>(),
            [("b", 1), ("a", 1)]
        );
        // Neither a huge value nor a huge unknown ID may flush useful updates.
        notification(&mut probe, "huge", json!("z".repeat(4096)))?;
        notification(&mut probe, &"z".repeat(4096), json!(1))?;
        assert_eq!(probe.supervisor.early_bytes, 2 * charge);
        probe.supervisor.dispatch(json!({"id":2,"result":"a"}));
        assert!(probe.supervisor.early.is_empty());
        assert_eq!(probe.supervisor.early_bytes, 0);
        let observed = probe.observe(None);
        assert_eq!(observed.subscriptions[1], ("a".into(), vec![Ok(payload)]));
        Ok(())
    }

    #[test]
    fn unsolicited_early_updates_require_a_live_transmitted_current_subscription_request()
    -> Result<(), RpcError> {
        for missing in ["absent", "unsent", "old_epoch", "cancelled", "ordinary"] {
            let mut probe = Probe::new()?;
            match missing {
                "absent" => {
                    probe.supervisor.pending.remove(&2);
                }
                "cancelled" => {
                    drop(probe.receivers.remove(1));
                }
                reason => {
                    if let Some(pending) = probe.supervisor.pending.get_mut(&2) {
                        match reason {
                            "unsent" => pending.sent = false,
                            "old_epoch" => pending.last_sent_epoch = 0,
                            _ => pending.subscription = None,
                        }
                    }
                }
            }
            notification(&mut probe, "unsolicited", json!("payload"))?;
            assert!(probe.supervisor.early.is_empty(), "{missing}");
            assert_eq!(probe.supervisor.early_bytes, 0, "{missing}");
            probe.observe(None);
        }
        Ok(())
    }

    #[test]
    fn early_accounting_resets_on_cancel_error_forget_and_close() -> Result<(), RpcError> {
        for ending in ["cancel", "error", "forget", "close"] {
            let mut probe = Probe::new()?;
            notification(&mut probe, "waiting", json!(42))?;
            assert!(probe.supervisor.early_bytes > 0);
            match ending {
                "cancel" => probe.supervisor.command(Some(Command::Cancel(vec![2]))),
                "error" => {
                    probe
                        .supervisor
                        .dispatch(json!({"id":2,"error":{"code":-1,"message":"failed"}}));
                }
                "forget" => probe.supervisor.remember_closed("waiting"),
                _ => probe.supervisor.finish(RpcError::Closed),
            }
            assert!(probe.supervisor.early.is_empty(), "{ending}");
            assert_eq!(probe.supervisor.early_bytes, 0, "{ending}");
            probe.observe(None);
        }
        Ok(())
    }

    #[test]
    fn reconnect_resets_early_storage_and_old_streams_share_the_active_byte_budget()
    -> Result<(), RpcError> {
        let mut probe = Probe::new()?;
        let charge = notification_bytes(&json!(7));
        probe.supervisor.subscriptions.clear();
        probe.supervisor.config.subscription_buffer_bytes = charge;
        probe.supervisor.subscription_budget = QueueBudget::new(charge);
        let old = Queue::new(2, probe.supervisor.subscription_budget.clone());
        probe.supervisor.subscriptions.insert(
            "old".into(),
            ActiveSubscription {
                queue: old.clone(),
                hash: None,
                request_id: 5,
                unsubscribe: "unsubscribe".into(),
            },
        );
        notification(&mut probe, "old", json!(7))?;
        notification(&mut probe, "waiting", json!(42))?;
        probe
            .supervisor
            .connection_lost(RpcError::Transport("lost".into()));
        assert_eq!(probe.supervisor.early_bytes, 0);
        assert!(probe.supervisor.early.is_empty());
        assert_eq!(
            probe.supervisor.subscription_budget.retained_bytes(),
            charge
        );
        let current = Queue::new(2, probe.supervisor.subscription_budget.clone());
        probe.supervisor.subscriptions.insert(
            "current".into(),
            ActiveSubscription {
                queue: current,
                hash: None,
                request_id: 6,
                unsubscribe: "unsubscribe".into(),
            },
        );
        notification(&mut probe, "current", json!(9))?;
        assert_eq!(
            probe.supervisor.subscription_budget.retained_bytes(),
            charge
        );
        let mut cx = Context::from_waker(Waker::noop());
        assert!(matches!(
            old.poll(&mut cx),
            Poll::Ready(Some(Err(WireError::Rpc(RpcError::SubscriptionLost))))
        ));
        probe.supervisor.finish(RpcError::Closed);
        assert_eq!(probe.supervisor.subscription_budget.retained_bytes(), 0);
        probe.observe(None);
        Ok(())
    }

    #[test]
    fn global_active_eviction_reports_the_victim_stream_and_keeps_newest_updates()
    -> Result<(), RpcError> {
        let mut probe = Probe::new()?;
        let charge = notification_bytes(&json!(1));
        probe.supervisor.subscriptions.clear();
        probe.supervisor.config.subscription_buffer_bytes = 2 * charge;
        probe.supervisor.subscription_budget = QueueBudget::new(2 * charge);
        for id in ["live", "7"] {
            probe.supervisor.subscriptions.insert(
                id.into(),
                ActiveSubscription {
                    queue: Queue::new(2, probe.supervisor.subscription_budget.clone()),
                    hash: None,
                    request_id: 5,
                    unsubscribe: "unsubscribe".into(),
                },
            );
        }
        let mut events = probe.supervisor.events.subscribe();
        notification(&mut probe, "live", json!(1))?;
        notification(&mut probe, "7", json!(2))?;
        notification(&mut probe, "7", json!(3))?;
        assert!(
            matches!(events.try_recv(), Ok(SessionEvent::SubscriptionOverflow { id }) if id == "live")
        );
        assert_eq!(
            probe.supervisor.subscription_budget.retained_bytes(),
            2 * charge
        );
        let observed = probe.observe(None);
        assert_eq!(
            observed.subscriptions,
            [
                ("7".into(), vec![Ok(json!(2)), Ok(json!(3))]),
                ("live".into(), vec![]),
            ]
        );
        Ok(())
    }

    #[test]
    fn early_drain_into_smaller_active_budget_reports_new_subscription_overflow()
    -> Result<(), RpcError> {
        let mut probe = Probe::new()?;
        let charge = notification_bytes(&json!(1));
        probe.supervisor.config.subscription_buffer_bytes = charge;
        probe.supervisor.subscription_budget = QueueBudget::new(charge);
        let mut events = probe.supervisor.events.subscribe();
        notification(&mut probe, "fresh", json!(1))?;
        notification(&mut probe, "fresh", json!(2))?;
        probe.supervisor.dispatch(json!({"id":2,"result":"fresh"}));
        assert!(
            matches!(events.try_recv(), Ok(SessionEvent::SubscriptionOverflow { id }) if id == "fresh")
        );
        assert_eq!(probe.supervisor.early_bytes, 0);
        let observed = probe.observe(None);
        assert_eq!(
            observed.subscriptions[1],
            ("fresh".into(), vec![Ok(json!(2))])
        );
        Ok(())
    }

    #[test]
    fn zero_retained_byte_budgets_are_refused_before_connecting() -> Result<(), RpcError> {
        let mut probe = Probe::new()?;
        probe.supervisor.config.early_notification_bytes = 0;
        assert!(matches!(
            validate(&probe.supervisor.config),
            Err(RpcError::Protocol(_))
        ));
        probe.supervisor.config.early_notification_bytes = 1024;
        probe.supervisor.config.subscription_buffer_bytes = 0;
        assert!(matches!(
            validate(&probe.supervisor.config),
            Err(RpcError::Protocol(_))
        ));
        probe.observe(None);
        Ok(())
    }

    #[derive(Clone, Default)]
    struct RawCapture(std::sync::Arc<std::sync::Mutex<Vec<String>>>);
    impl tracing::Subscriber for RawCapture {
        fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
            metadata.target() == "rtao::rpc::raw"
        }
        fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
            tracing::span::Id::from_u64(1)
        }
        fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
        fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
        fn event(&self, event: &tracing::Event<'_>) {
            struct Visitor<'a>(&'a RawCapture);
            impl tracing::field::Visit for Visitor<'_> {
                fn record_debug(
                    &mut self,
                    field: &tracing::field::Field,
                    value: &dyn std::fmt::Debug,
                ) {
                    if field.name() == "frame"
                        && let Ok(mut frames) = self.0.0.lock()
                    {
                        frames.push(format!("{value:?}"));
                    }
                }
            }
            event.record(&mut Visitor(self));
        }
        fn enter(&self, _: &tracing::span::Id) {}
        fn exit(&self, _: &tracing::span::Id) {}
    }

    #[test]
    fn raw_receive_logs_escape_terminal_controls_while_protocol_values_stay_exact()
    -> Result<(), RpcError> {
        let mut probe = Probe::new()?;
        let capture = RawCapture::default();
        tracing::subscriber::with_default(capture.clone(), || -> Result<(), RpcError> {
            probe.supervisor.inbound(Message::Text(
                "\u{1b}]52;c;payload\u{7}\r\nmalformed".into(),
            ))?;
            let control = "\u{9b}31m";
            let raw = format!(r#"{{"params":{{"subscription":"live","result":"{control}"}}}}"#);
            probe.supervisor.inbound(Message::Text(raw.into()))?;
            Ok(())
        })?;
        let frames = capture
            .0
            .lock()
            .map_err(|_| RpcError::Protocol("capture lock poisoned".into()))?;
        assert_eq!(frames.len(), 2);
        assert!(
            frames
                .iter()
                .all(|frame| !frame.chars().any(char::is_control))
        );
        assert!(frames[0].contains(r"\u{1b}]52;c;payload\u{7}\r\n"));
        assert!(frames[1].contains(r"\u{9b}31m"));
        let observed = probe.observe(None);
        assert_eq!(
            observed.subscriptions[1],
            ("live".into(), vec![Ok(json!("\u{9b}31m"))])
        );
        Ok(())
    }
}
