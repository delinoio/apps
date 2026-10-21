// SPDX-License-Identifier: FSL-1.1-ALv2
//! Prometheus metrics with bounded labels. Tenants, models, request IDs, key IDs, error messages
//! and any content never become label values. Every label below is a closed enum.
use crate::store::recovery::Backlog;
use prometheus::{
    Encoder, HistogramOpts, HistogramVec, IntCounter, IntCounterVec, IntGauge, Opts, Registry,
    TextEncoder,
};

/// RPC names. Labels stay bounded by this enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Procedure {
    TranslateTextArray,
    ExtractDataFromText,
    AssociateGeneration,
    CreateKey,
    ListKeys,
    RevokeKey,
    GetSettings,
    UpdateSettings,
    InspectRequest,
}
impl Procedure {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TranslateTextArray => "TranslateTextArray",
            Self::ExtractDataFromText => "ExtractDataFromText",
            Self::AssociateGeneration => "AssociateGeneration",
            Self::CreateKey => "CreateKey",
            Self::ListKeys => "ListKeys",
            Self::RevokeKey => "RevokeKey",
            Self::GetSettings => "GetSettings",
            Self::UpdateSettings => "UpdateSettings",
            Self::InspectRequest => "InspectRequest",
        }
    }
}

/// Failure categories counted without per-tenant detail.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailureKind {
    /// A credential could not be authenticated.
    Authentication,
    /// A user or key lacks current permission.
    Authorization,
    /// The tenant authority or billing check could not answer; new paid work failed closed.
    AuthorityUnavailable,
    /// Gateway or model provider failure (recorded by the execution layer).
    Provider,
    /// Catalog price synchronization failure (recorded by the execution layer).
    CatalogSync,
    /// A request was rejected because no price exists (recorded by the execution layer).
    MissingPrice,
    /// Usage delivery to delcore failed (recorded by the execution layer).
    UsageDelivery,
    /// A background task run failed.
    Worker,
}
impl FailureKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Authentication => "authentication",
            Self::Authorization => "authorization",
            Self::AuthorityUnavailable => "authority_unavailable",
            Self::Provider => "provider",
            Self::CatalogSync => "catalog_sync",
            Self::MissingPrice => "missing_price",
            Self::UsageDelivery => "usage_delivery",
            Self::Worker => "worker",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LimitKind {
    TenantRate,
    TenantConcurrency,
    InputSize,
    ItemCount,
}
impl LimitKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TenantRate => "tenant_rate",
            Self::TenantConcurrency => "tenant_concurrency",
            Self::InputSize => "input_size",
            Self::ItemCount => "item_count",
        }
    }
}

pub struct Metrics {
    registry: Registry,
    pub requests: IntCounterVec,
    pub duration: HistogramVec,
    pub failures: IntCounterVec,
    pub limits: IntCounterVec,
    pub retention_deleted: IntCounterVec,
    pub retention_runs: IntCounter,
    pub task_runs: IntCounterVec,
    unknown_usage: IntGauge,
    pending_delivery: IntGauge,
    unknown_requests: IntGauge,
    nearing_expiry: IntGauge,
    price_available: IntGauge,
    backlog_alert: IntGauge,
    expiry_alert: IntGauge,
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
            let value = IntCounterVec::new(Opts::new(name, help), labels)
                .expect("static metric definition");
            registry
                .register(Box::new(value.clone()))
                .expect("unique metric name");
            value
        };
        let gauge = |name: &str, help: &str| {
            let value = IntGauge::new(name, help).expect("static metric definition");
            registry
                .register(Box::new(value.clone()))
                .expect("unique metric name");
            value
        };
        let duration = HistogramVec::new(
            HistogramOpts::new(
                "delitools_request_duration_seconds",
                "RPC latency by procedure.",
            ),
            &["procedure"],
        )
        .expect("static metric definition");
        registry
            .register(Box::new(duration.clone()))
            .expect("unique metric name");
        let retention_runs = IntCounter::new(
            "delitools_retention_runs_total",
            "Completed retention sweeps.",
        )
        .expect("static metric definition");
        registry
            .register(Box::new(retention_runs.clone()))
            .expect("unique metric name");
        Self {
            requests: counter(
                "delitools_requests_total",
                "RPC results by procedure and bounded error reason.",
                &["procedure", "outcome"],
            ),
            duration,
            failures: counter(
                "delitools_failures_total",
                "Operational failures by bounded kind.",
                &["kind"],
            ),
            limits: counter(
                "delitools_limit_rejections_total",
                "Requests rejected by an operator-configured limit.",
                &["kind"],
            ),
            retention_deleted: counter(
                "delitools_retention_deleted_total",
                "Records deleted at the 30-day boundary by table.",
                &["table"],
            ),
            retention_runs,
            task_runs: counter(
                "delitools_worker_runs_total",
                "Background task runs by task and outcome.",
                &["task", "outcome"],
            ),
            unknown_usage: gauge(
                "delitools_attempts_unknown_usage",
                "Attempts whose measured usage is unknown.",
            ),
            pending_delivery: gauge(
                "delitools_attempts_pending_delivery",
                "Attempts with known usage that are not delivered.",
            ),
            unknown_requests: gauge(
                "delitools_requests_unknown_outcome",
                "Requests with an unknown outcome.",
            ),
            nearing_expiry: gauge(
                "delitools_attempts_nearing_expiry",
                "Undelivered attempts inside the expiry alert window.",
            ),
            price_available: gauge(
                "delitools_catalog_price_available",
                "1 when at least one synchronized catalog price exists.",
            ),
            backlog_alert: gauge(
                "delitools_backlog_alert",
                "1 when pending attempts reach the configured alert threshold.",
            ),
            expiry_alert: gauge(
                "delitools_expiry_alert",
                "1 when undelivered attempts are inside the expiry alert window.",
            ),
            registry,
        }
    }

    pub fn failure(&self, kind: FailureKind) {
        self.failures.with_label_values(&[kind.as_str()]).inc();
    }

    pub fn limit(&self, kind: LimitKind) {
        self.limits.with_label_values(&[kind.as_str()]).inc();
    }

    /// Publish the sampled backlog and evaluate the alert thresholds. Returns the alert flags
    /// (backlog, expiry).
    pub fn set_backlog(&self, backlog: &Backlog, alert_backlog: u32) -> (bool, bool) {
        self.unknown_usage.set(backlog.unknown_usage);
        self.pending_delivery.set(backlog.pending_delivery);
        self.unknown_requests.set(backlog.unknown_requests);
        self.nearing_expiry.set(backlog.nearing_expiry);
        self.price_available.set(i64::from(backlog.price_available));
        let backlog_alert =
            backlog.unknown_usage + backlog.pending_delivery >= i64::from(alert_backlog);
        let expiry_alert = backlog.nearing_expiry > 0;
        self.backlog_alert.set(i64::from(backlog_alert));
        self.expiry_alert.set(i64::from(expiry_alert));
        (backlog_alert, expiry_alert)
    }

    pub fn encode(&self) -> String {
        let mut out = Vec::new();
        TextEncoder::new()
            .encode(&self.registry.gather(), &mut out)
            .expect("encode registered metrics");
        String::from_utf8(out).expect("prometheus text is UTF-8")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_are_closed_and_never_carry_identifiers() {
        let metrics = Metrics::new();
        metrics
            .requests
            .with_label_values(&[Procedure::CreateKey.as_str(), "ok"])
            .inc();
        metrics.failure(FailureKind::AuthorityUnavailable);
        metrics.limit(LimitKind::TenantRate);
        let text = metrics.encode();
        assert!(
            text.contains("delitools_requests_total{outcome=\"ok\",procedure=\"CreateKey\"} 1")
        );
        assert!(text.contains("delitools_failures_total{kind=\"authority_unavailable\"} 1"));
        for forbidden in ["tenant_id", "request_id", "model=", "user"] {
            assert!(!text.contains(forbidden), "{forbidden}");
        }
    }

    #[test]
    fn backlog_gauges_and_alerts_follow_thresholds() {
        let metrics = Metrics::new();
        let backlog = Backlog {
            unknown_usage: 2,
            pending_delivery: 3,
            unknown_requests: 1,
            nearing_expiry: 0,
            price_available: true,
        };
        assert_eq!(metrics.set_backlog(&backlog, 6), (false, false));
        assert_eq!(metrics.set_backlog(&backlog, 5), (true, false));
        let near = Backlog {
            nearing_expiry: 1,
            ..backlog
        };
        assert_eq!(metrics.set_backlog(&near, 100), (false, true));
        let text = metrics.encode();
        assert!(text.contains("delitools_catalog_price_available 1"));
        assert!(text.contains("delitools_attempts_nearing_expiry 1"));
    }
}
