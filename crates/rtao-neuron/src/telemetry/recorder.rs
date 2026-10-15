//! Adapter collecting the chain RPC metric family into the process listener.

use super::Metrics;
use metrics::{
    Counter, CounterFn, Gauge, GaugeFn, Histogram, HistogramFn, Key, KeyName, Metadata,
    SharedString, Unit,
};
use std::sync::Arc;

pub(super) struct Recorder(pub(super) Metrics);
impl metrics::Recorder for Recorder {
    fn describe_counter(&self, _: KeyName, _: Option<Unit>, _: SharedString) {}
    fn describe_gauge(&self, _: KeyName, _: Option<Unit>, _: SharedString) {}
    fn describe_histogram(&self, _: KeyName, _: Option<Unit>, _: SharedString) {}
    fn register_counter(&self, key: &Key, _: &Metadata<'_>) -> Counter {
        Counter::from_arc(Arc::new(Handle {
            metrics: self.0.clone(),
            key: render_key(key),
            sum: render_named_key(key, "_sum"),
            count: render_named_key(key, "_count"),
        }))
    }
    fn register_gauge(&self, key: &Key, _: &Metadata<'_>) -> Gauge {
        Gauge::from_arc(Arc::new(Handle {
            metrics: self.0.clone(),
            key: render_key(key),
            sum: render_named_key(key, "_sum"),
            count: render_named_key(key, "_count"),
        }))
    }
    fn register_histogram(&self, key: &Key, _: &Metadata<'_>) -> Histogram {
        Histogram::from_arc(Arc::new(Handle {
            metrics: self.0.clone(),
            key: render_key(key),
            sum: render_named_key(key, "_sum"),
            count: render_named_key(key, "_count"),
        }))
    }
}
struct Handle {
    metrics: Metrics,
    key: String,
    sum: String,
    count: String,
}
impl CounterFn for Handle {
    fn increment(&self, value: u64) {
        let mut inner = self.metrics.0.lock().unwrap_or_else(|p| p.into_inner());
        let current = inner.counters.entry(self.key.clone()).or_default();
        *current = current.saturating_add(value);
    }
    fn absolute(&self, value: u64) {
        let mut inner = self.metrics.0.lock().unwrap_or_else(|p| p.into_inner());
        let current = inner.counters.entry(self.key.clone()).or_default();
        *current = (*current).max(value);
    }
}
impl GaugeFn for Handle {
    fn increment(&self, value: f64) {
        let mut inner = self.metrics.0.lock().unwrap_or_else(|p| p.into_inner());
        let current = inner.gauges.entry(self.key.clone()).or_default();
        let next = *current + value;
        if next.is_finite() {
            *current = next;
        }
    }
    fn decrement(&self, value: f64) {
        GaugeFn::increment(self, -value);
    }
    fn set(&self, value: f64) {
        self.metrics.gauge(&self.key, value);
    }
}
impl HistogramFn for Handle {
    fn record(&self, value: f64) {
        if !value.is_finite() {
            return;
        }
        let mut inner = self.metrics.0.lock().unwrap_or_else(|p| p.into_inner());
        let sum = inner.gauges.entry(self.sum.clone()).or_default();
        let next = *sum + value;
        if !next.is_finite() {
            return;
        }
        *sum = next;
        let count = inner.counters.entry(self.count.clone()).or_default();
        *count = count.saturating_add(1);
    }
}
fn render_key(key: &Key) -> String {
    render_named_key(key, "")
}
fn render_named_key(key: &Key, suffix: &str) -> String {
    let mut rendered = format!("{}{suffix}", key.name());
    let mut labels = key.labels().collect::<Vec<_>>();
    labels.sort_by_key(|label| label.key());
    if !labels.is_empty() {
        rendered.push('{');
        for (index, label) in labels.into_iter().enumerate() {
            if index > 0 {
                rendered.push(',');
            }
            rendered.push_str(label.key());
            rendered.push_str("=\"");
            for character in label.value().chars() {
                match character {
                    '\\' => rendered.push_str("\\\\"),
                    '"' => rendered.push_str("\\\""),
                    '\n' => rendered.push_str("\\n"),
                    character => rendered.push(character),
                }
            }
            rendered.push('"');
        }
        rendered.push('}');
    }
    rendered
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rpc_metrics_are_retained_with_valid_labels_and_histogram_totals() {
        let metrics = Metrics::default();
        let recorder = Recorder(metrics.clone());
        metrics::with_local_recorder(&recorder, || {
            metrics::counter!("rtao_rpc_requests_total", "method"=>"storage", "endpoint"=>"a\"b\\c\nd").increment(3);
            metrics::counter!("rtao_rpc_requests_total", "method"=>"storage", "endpoint"=>"a\"b\\c\nd").absolute(2);
            metrics::gauge!("rtao_rpc_inflight").increment(2.0);
            metrics::gauge!("rtao_rpc_inflight").decrement(1.0);
            metrics::histogram!("rtao_rpc_request_seconds", "method"=>"storage").record(0.2);
            metrics::histogram!("rtao_rpc_request_seconds", "method"=>"storage").record(0.3);
        });
        let output = metrics.render();
        assert!(output.contains(
            "rtao_rpc_requests_total{endpoint=\"a\\\"b\\\\c\\nd\",method=\"storage\"} 3\n"
        ));
        assert!(output.contains("rtao_rpc_inflight 1\n"));
        assert!(output.contains("rtao_rpc_request_seconds_count{method=\"storage\"} 2\n"));
        assert!(output.contains("rtao_rpc_request_seconds_sum{method=\"storage\"} 0.5\n"));
    }
}
