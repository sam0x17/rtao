//! Recorder-level verification of transport telemetry across concurrent sessions.
#![cfg(feature = "rpc-metrics")]
use metrics::{
    Counter, CounterFn, Gauge, GaugeFn, Histogram, HistogramFn, Key, KeyName, Metadata, Recorder,
    SharedString, Unit,
};
use rtao_rpc::{ReadMethod, RpcError, Session, SessionConfig};
use rtao_testkit::{FakeNode, Reply};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    error::Error,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::time::timeout;
use url::Url;
type Result<T = ()> = std::result::Result<T, Box<dyn Error + Send + Sync>>;
#[derive(Default)]
struct Samples {
    gauge: f64,
    counters: BTreeMap<String, u64>,
    histograms: BTreeMap<String, usize>,
}
struct Handle {
    key: String,
    samples: Arc<Mutex<Samples>>,
}
impl CounterFn for Handle {
    fn increment(&self, value: u64) {
        if let Ok(mut samples) = self.samples.lock() {
            *samples.counters.entry(self.key.clone()).or_default() += value;
        }
    }
    fn absolute(&self, value: u64) {
        if let Ok(mut samples) = self.samples.lock() {
            samples.counters.insert(self.key.clone(), value);
        }
    }
}
impl GaugeFn for Handle {
    fn increment(&self, value: f64) {
        if let Ok(mut samples) = self.samples.lock() {
            samples.gauge += value;
        }
    }
    fn decrement(&self, value: f64) {
        if let Ok(mut samples) = self.samples.lock() {
            samples.gauge -= value;
        }
    }
    fn set(&self, value: f64) {
        if let Ok(mut samples) = self.samples.lock() {
            samples.gauge = value;
        }
    }
}
impl HistogramFn for Handle {
    fn record(&self, _value: f64) {
        if let Ok(mut samples) = self.samples.lock() {
            *samples.histograms.entry(self.key.clone()).or_default() += 1;
        }
    }
}
struct Capture(Arc<Mutex<Samples>>);
fn metric_key(key: &Key) -> String {
    format!("{} {:?}", key.name(), key.labels().collect::<Vec<_>>())
}
impl Recorder for Capture {
    fn describe_counter(&self, _key: KeyName, _unit: Option<Unit>, _description: SharedString) {}
    fn describe_gauge(&self, _key: KeyName, _unit: Option<Unit>, _description: SharedString) {}
    fn describe_histogram(&self, _key: KeyName, _unit: Option<Unit>, _description: SharedString) {}
    fn register_counter(&self, key: &Key, _metadata: &Metadata<'_>) -> Counter {
        Counter::from_arc(Arc::new(Handle {
            key: metric_key(key),
            samples: self.0.clone(),
        }))
    }
    fn register_gauge(&self, key: &Key, _metadata: &Metadata<'_>) -> Gauge {
        Gauge::from_arc(Arc::new(Handle {
            key: metric_key(key),
            samples: self.0.clone(),
        }))
    }
    fn register_histogram(&self, key: &Key, _metadata: &Metadata<'_>) -> Histogram {
        Histogram::from_arc(Arc::new(Handle {
            key: metric_key(key),
            samples: self.0.clone(),
        }))
    }
}
async fn gauge(samples: &Arc<Mutex<Samples>>, expected: f64) -> Result {
    timeout(Duration::from_secs(1), async {
        loop {
            if samples
                .lock()
                .map(|state| state.gauge == expected)
                .unwrap_or(false)
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await?;
    Ok(())
}
#[tokio::test]
async fn aggregate_inflight_and_completion_metrics_cover_cancel_close_and_policy() -> Result {
    let samples = Arc::new(Mutex::new(Samples::default()));
    metrics::set_global_recorder(Capture(samples.clone()))?;
    let first = FakeNode::start().await?;
    let second = FakeNode::start().await?;
    for node in [&first, &second] {
        node.mutate_state(|state| state.set_handler("system_chain", |_, _| Reply::silent()));
    }
    let config = |node: &FakeNode| -> Result<SessionConfig> {
        Ok(SessionConfig {
            silence_timeout: Duration::from_secs(30),
            ..SessionConfig::new(Url::parse(node.url())?)
        })
    };
    let session1 = Session::connect(config(&first)?).await?;
    let session2 = Session::connect(config(&second)?).await?;
    let request1 = {
        let session = session1.clone();
        tokio::spawn(async move {
            session
                .request::<Value>(ReadMethod::SystemChain, json!([]))
                .await
        })
    };
    let request2 = {
        let session = session2.clone();
        tokio::spawn(async move {
            session
                .request::<Value>(ReadMethod::SystemChain, json!([]))
                .await
        })
    };
    gauge(&samples, 2.0).await?;
    request1.abort();
    assert!(
        request1
            .await
            .err()
            .is_some_and(|error| error.is_cancelled())
    );
    gauge(&samples, 1.0).await?;
    session2.close().await;
    assert!(matches!(request2.await?, Err(RpcError::Closed)));
    gauge(&samples, 0.0).await?;
    session1.close().await;
    let refused = FakeNode::start().await?;
    refused.mutate_state(|state| {
        state.set_handler("system_chain", |_, _| {
            Reply::error(-32004, "storage work rate limit exceeded", None)
        })
    });
    let session = Session::connect(config(&refused)?).await?;
    assert!(matches!(
        session
            .request::<Value>(ReadMethod::SystemChain, json!([]))
            .await,
        Err(RpcError::Policy(_))
    ));
    gauge(&samples, 0.0).await?;
    {
        let state = samples.lock().map_err(|_| "metric capture poisoned")?;
        let count = |needle: &str| {
            state
                .counters
                .iter()
                .filter(|(key, _)| key.contains(needle))
                .map(|(_, value)| value)
                .sum::<u64>()
        };
        assert_eq!(count("cancelled"), 1);
        assert_eq!(count("closed"), 1);
        assert_eq!(count("rtao_rpc_policy_refusals_total"), 1);
        assert_eq!(
            state
                .histograms
                .iter()
                .filter(|(key, _)| key.starts_with("rtao_rpc_request_seconds"))
                .map(|(_, count)| count)
                .sum::<usize>(),
            3
        );
    }
    session.close().await;
    Ok(())
}
