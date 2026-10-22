// SPDX-License-Identifier: FSL-1.1-ALv2
//! Prometheus metrics with bounded labels.
//!
//! Labels come from closed sets (procedure, error reason, outcome categories). They never carry
//! tenant IDs, request IDs, models supplied by callers, text, schemas, outputs or credentials.

use prometheus::{
    Encoder, HistogramOpts, HistogramVec, IntCounter, IntCounterVec, IntGauge, Opts, Registry,
    TextEncoder,
};

/// RPC procedures; the label set of the `procedure` metric dimension.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Procedure {
    Translate,
    Extract,
}

impl Procedure {
    pub fn name(self) -> &'static str {
        match self {
            Self::Translate => "Translate",
            Self::Extract => "Extract",
        }
    }
    /// Delcore capability checked for the procedure.
    pub fn capability(self) -> &'static str {
        match self {
            Self::Translate => "translate",
            Self::Extract => "extract",
        }
    }
}

pub struct Metrics {
    registry: Registry,
    pub requests: IntCounterVec,
    pub duration: HistogramVec,
    /// Calls rejected before execution, by error reason.
    pub denied: IntCounterVec,
    /// Executions that failed after starting, by procedure and error reason.
    pub failures: IntCounterVec,
    /// Gateway attempts by outcome: ok, transient, permanent, invalid_output, canceled, timeout.
    pub gateway_attempts: IntCounterVec,
    /// Attempts whose provider usage could not be confirmed, by reason.
    pub unknown_usage: IntCounterVec,
    /// Confirmed usage events persisted, by direction.
    pub usage_persisted: IntCounterVec,
    pub persist_failures: IntCounter,
    /// Delivery results: delivered, retry, poison, tenant_deleted.
    pub deliveries: IntCounterVec,
    pub expired: IntCounter,
    pub outbox_pending: IntGauge,
    pub outbox_poisoned: IntGauge,
    pub outbox_oldest_pending_age_seconds: IntGauge,
    /// 1 while the backlog or age alert threshold is exceeded.
    pub outbox_alert: IntGauge,
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

impl Metrics {
    pub fn new() -> Self {
        let registry = Registry::new();
        let counter = |name: &str, help: &str, labels: &[&str]| {
            let c = IntCounterVec::new(Opts::new(name, help), labels).unwrap();
            registry.register(Box::new(c.clone())).unwrap();
            c
        };
        let plain = |name: &str, help: &str| {
            let c = IntCounter::new(name, help).unwrap();
            registry.register(Box::new(c.clone())).unwrap();
            c
        };
        let gauge = |name: &str, help: &str| {
            let g = IntGauge::new(name, help).unwrap();
            registry.register(Box::new(g.clone())).unwrap();
            g
        };
        let duration = HistogramVec::new(
            HistogramOpts::new(
                "delitools_request_duration_seconds",
                "Request latency by procedure.",
            ),
            &["procedure"],
        )
        .unwrap();
        registry.register(Box::new(duration.clone())).unwrap();
        Self {
            requests: counter(
                "delitools_requests_total",
                "Requests by procedure and outcome.",
                &["procedure", "outcome"],
            ),
            denied: counter(
                "delitools_denied_total",
                "Rejected calls by error reason.",
                &["reason"],
            ),
            failures: counter(
                "delitools_execution_failures_total",
                "Failed executions.",
                &["procedure", "reason"],
            ),
            gateway_attempts: counter(
                "delitools_gateway_attempts_total",
                "Gateway attempts by outcome.",
                &["outcome"],
            ),
            unknown_usage: counter(
                "delitools_unknown_usage_total",
                "Attempts without confirmed usage.",
                &["reason"],
            ),
            usage_persisted: counter(
                "delitools_usage_persisted_total",
                "Usage events stored for delivery.",
                &["direction"],
            ),
            persist_failures: plain(
                "delitools_usage_persist_failures_total",
                "Confirmed usage that could not be stored.",
            ),
            deliveries: counter(
                "delitools_usage_deliveries_total",
                "Usage delivery results.",
                &["outcome"],
            ),
            expired: plain(
                "delitools_usage_expired_total",
                "Usage rows removed at the 30-day expiry.",
            ),
            outbox_pending: gauge(
                "delitools_outbox_pending",
                "Usage rows waiting for delivery.",
            ),
            outbox_poisoned: gauge(
                "delitools_outbox_poisoned",
                "Usage rows Delcore will never accept.",
            ),
            outbox_oldest_pending_age_seconds: gauge(
                "delitools_outbox_oldest_pending_age_seconds",
                "Age of the oldest pending usage row.",
            ),
            outbox_alert: gauge(
                "delitools_outbox_alert",
                "1 when delivery backlog thresholds are exceeded.",
            ),
            duration,
            registry,
        }
    }

    pub fn encode(&self) -> String {
        let mut out = Vec::new();
        TextEncoder::new()
            .encode(&self.registry.gather(), &mut out)
            .expect("encode registered metrics");
        String::from_utf8(out).expect("metrics are UTF-8")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metrics_encode_with_bounded_labels() {
        let metrics = Metrics::new();
        metrics
            .requests
            .with_label_values(&["Translate", "ok"])
            .inc();
        metrics.denied.with_label_values(&["unauthenticated"]).inc();
        metrics.outbox_pending.set(3);
        let text = metrics.encode();
        assert!(
            text.contains("delitools_requests_total{outcome=\"ok\",procedure=\"Translate\"} 1")
        );
        assert!(text.contains("delitools_outbox_pending 3"));
    }
}
