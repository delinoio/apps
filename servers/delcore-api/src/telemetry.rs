// SPDX-License-Identifier: FSL-1.1-ALv2
//! Bounded operational metrics. Identity values and error messages never become labels.
use prometheus::{
    Encoder, Gauge, Histogram, HistogramOpts, HistogramVec, IntCounterVec, Opts, Registry,
    TextEncoder,
};
use sqlx::{PgPool, Row};
use std::time::Duration;

#[derive(Clone, Copy, Debug)]
pub enum Outcome {
    Applied,
    Unchanged,
    Rejected,
    Accepted,
    Replayed,
    Deferred,
    Failed,
}
impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Applied => "applied",
            Self::Unchanged => "unchanged",
            Self::Rejected => "rejected",
            Self::Accepted => "accepted",
            Self::Replayed => "replayed",
            Self::Deferred => "deferred",
            Self::Failed => "failed",
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub enum CorrectionKind {
    Replace,
    Cancel,
    Invalid,
}
impl CorrectionKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Replace => "replace",
            Self::Cancel => "cancel",
            Self::Invalid => "invalid",
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub enum LimitKind {
    Groups,
    DistinctValues,
}
impl LimitKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Groups => "groups",
            Self::DistinctValues => "distinct_values",
        }
    }
}

pub fn reason(value: Option<&str>) -> &'static str {
    match value {
        None => "none",
        Some("invalid_input") => "invalid_input",
        Some("unauthenticated") => "unauthenticated",
        Some("forbidden") => "forbidden",
        Some("conflict") => "conflict",
        Some("precondition") => "precondition",
        Some("dependency") => "dependency",
        Some("stale_revision") => "stale_revision",
        Some("no_matching_meter") => "no_matching_meter",
        Some("invalid_attribute") => "invalid_attribute",
        Some("cardinality_exceeded") => "cardinality_exceeded",
        Some("event_identity_conflict") => "event_identity_conflict",
        Some("event_not_found") => "event_not_found",
        Some("event_cancelled") => "event_cancelled",
        Some("reporting_window_closed") => "reporting_window_closed",
        _ => "other",
    }
}

#[derive(Clone)]
pub struct Metrics {
    registry: Registry,
    catalog: IntCounterVec,
    catalog_duration: Histogram,
    ingestions: IntCounterVec,
    corrections: IntCounterVec,
    cardinality: IntCounterVec,
    metering: IntCounterVec,
    metering_duration: HistogramVec,
    retries: IntCounterVec,
    gauges: Vec<Gauge>,
}
impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}
impl Metrics {
    pub fn new() -> Self {
        let registry = Registry::new();
        let counter = |name, help, labels: &[&str]| {
            let value = IntCounterVec::new(Opts::new(name, help), labels)
                .expect("static metric definition");
            registry
                .register(Box::new(value.clone()))
                .expect("unique metric name");
            value
        };
        let catalog = counter(
            "delcore_catalog_reconciliations_total",
            "Development catalog applications by outcome.",
            &["outcome"],
        );
        let ingestions = counter(
            "delcore_metering_ingestions_total",
            "Event ingestion requests by outcome and typed error reason.",
            &["outcome", "reason"],
        );
        let corrections = counter(
            "delcore_metering_corrections_total",
            "Event correction requests by kind, outcome, and typed error reason.",
            &["kind", "outcome", "reason"],
        );
        let cardinality = counter(
            "delcore_metering_cardinality_rejections_total",
            "Events rejected by a cardinality admission bound.",
            &["limit"],
        );
        let metering = counter(
            "delcore_metering_jobs_total",
            "Metering job attempts by outcome and failure category.",
            &["outcome", "category"],
        );
        let retries = counter(
            "delcore_operator_job_retries_total",
            "Failed jobs returned to processing by an operator, by job kind.",
            &["kind"],
        );
        let catalog_duration = Histogram::with_opts(HistogramOpts::new(
            "delcore_catalog_reconciliation_duration_seconds",
            "Duration of development catalog applications.",
        ))
        .expect("static histogram");
        let metering_duration = HistogramVec::new(
            HistogramOpts::new(
                "delcore_metering_processing_duration_seconds",
                "Duration of metering job attempts by outcome.",
            ),
            &["outcome"],
        )
        .expect("static histogram");
        registry
            .register(Box::new(catalog_duration.clone()))
            .expect("unique histogram");
        registry
            .register(Box::new(metering_duration.clone()))
            .expect("unique histogram");
        let gauges = [
            ("delcore_job_backlog", "Pending or reconciliation jobs."),
            ("delcore_failed_jobs", "Jobs requiring operator attention."),
            (
                "delcore_provider_mismatches",
                "Provider payments requiring attribution review.",
            ),
            ("delcore_email_failures", "Failed email delivery jobs."),
            (
                "delcore_retry_exhausted",
                "Jobs that exhausted automatic retries.",
            ),
            (
                "delcore_settlement_delay_seconds",
                "Age of the oldest incomplete settlement.",
            ),
            (
                "delcore_metering_backlog",
                "Accepted metering work awaiting aggregate materialization.",
            ),
            (
                "delcore_metering_failed_jobs",
                "Metering work requiring operator retry.",
            ),
            (
                "delcore_metering_lag_seconds",
                "Age of the oldest pending metering work.",
            ),
        ]
        .into_iter()
        .map(|(name, help)| {
            let gauge = Gauge::new(name, help).expect("static gauge");
            registry
                .register(Box::new(gauge.clone()))
                .expect("unique gauge");
            gauge
        })
        .collect();
        Self {
            registry,
            catalog,
            catalog_duration,
            ingestions,
            corrections,
            cardinality,
            metering,
            metering_duration,
            retries,
            gauges,
        }
    }
    pub fn encode(&self) -> String {
        let mut bytes = Vec::new();
        TextEncoder::new()
            .encode(&self.registry.gather(), &mut bytes)
            .expect("encode registered metrics");
        String::from_utf8(bytes).expect("Prometheus UTF-8")
    }
    pub fn catalog_applied(&self, outcome: Outcome, elapsed: Duration) {
        self.catalog.with_label_values(&[outcome.as_str()]).inc();
        self.catalog_duration.observe(elapsed.as_secs_f64());
    }
    pub fn event_ingested(&self, outcome: Outcome, error_reason: Option<&str>) {
        self.ingestions
            .with_label_values(&[outcome.as_str(), reason(error_reason)])
            .inc();
    }
    pub fn event_corrected(
        &self,
        kind: CorrectionKind,
        outcome: Outcome,
        error_reason: Option<&str>,
    ) {
        self.corrections
            .with_label_values(&[kind.as_str(), outcome.as_str(), reason(error_reason)])
            .inc();
    }
    pub fn cardinality_rejected(&self, kind: LimitKind) {
        self.cardinality.with_label_values(&[kind.as_str()]).inc();
    }
    pub fn metering_job(&self, outcome: Outcome, category: &str, elapsed: Duration) {
        let category = match category {
            "" => "none",
            "metering_transient" | "invalid_payload" | "metering_invalid" | "retry_exhausted" => {
                category
            }
            _ => "other",
        };
        self.metering
            .with_label_values(&[outcome.as_str(), category])
            .inc();
        self.metering_duration
            .with_label_values(&[outcome.as_str()])
            .observe(elapsed.as_secs_f64());
    }
    pub fn operator_retry(&self, kind: &str) {
        let kind = match kind {
            "email" | "webhook" | "metering" | "topup" | "auto_topup" | "settlement"
            | "subscription" | "cancel" => kind,
            _ => "other",
        };
        self.retries.with_label_values(&[kind]).inc();
    }
    /// Sample all gauges from one database snapshot; failures preserve the previous values.
    /// The caller bounds this query with its configured request timeout.
    pub async fn sample_jobs(
        &self,
        pool: &PgPool,
        backlog_alert: i64,
        settlement_alert_seconds: f64,
    ) -> Result<(), sqlx::Error> {
        let row = sqlx::query(r#"SELECT
            count(*) FILTER(WHERE state IN ('pending','reconcile'))::float8,
            count(*) FILTER(WHERE state='failed')::float8,
            count(*) FILTER(WHERE error_category IN ('payment_mismatch','unknown_payment_outcome'))::float8,
            count(*) FILTER(WHERE kind='email' AND state='failed')::float8,
            count(*) FILTER(WHERE error_category='retry_exhausted')::float8,
            COALESCE(EXTRACT(EPOCH FROM now()-min(created_at) FILTER(WHERE kind='settlement' AND state<>'done')),0)::float8,
            count(*) FILTER(WHERE kind='metering' AND state='pending')::float8,
            count(*) FILTER(WHERE kind='metering' AND state='failed')::float8,
            COALESCE(EXTRACT(EPOCH FROM now()-min(created_at) FILTER(WHERE kind='metering' AND state='pending')),0)::float8
            FROM jobs"#).fetch_one(pool).await?;
        let values: Vec<f64> = (0..9)
            .map(|index| row.try_get(index))
            .collect::<Result<_, _>>()?;
        for (gauge, value) in self.gauges.iter().zip(&values) {
            gauge.set(*value);
        }
        if values[0] >= backlog_alert as f64 {
            tracing::warn!(category = "job_backlog", "job backlog threshold exceeded");
        }
        if values[5] >= settlement_alert_seconds {
            tracing::warn!(
                category = "settlement_delay",
                "settlement delay threshold exceeded"
            );
        }
        if values[7] > 0.0 {
            tracing::warn!(
                category = "metering_failure",
                failed = values[7],
                "metering work requires operator retry"
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn labels_reject_identifiers_and_raw_errors() {
        let metrics = Metrics::default();
        metrics.metering_job(Outcome::Failed, "tenant-secret", Duration::from_millis(1));
        metrics.metering_job(Outcome::Applied, "", Duration::from_millis(2));
        metrics.operator_retry("tenant-secret");
        metrics.operator_retry("metering");
        metrics.event_ingested(Outcome::Rejected, Some("tenant-secret"));
        let text = metrics.encode();
        assert!(!text.contains("tenant-secret"));
        assert!(
            text.contains("delcore_metering_jobs_total{category=\"other\",outcome=\"failed\"} 1")
        );
        assert!(
            text.contains("delcore_metering_jobs_total{category=\"none\",outcome=\"applied\"} 1")
        );
        assert!(text.contains("delcore_operator_job_retries_total{kind=\"other\"} 1"));
        assert!(text.contains("delcore_operator_job_retries_total{kind=\"metering\"} 1"));
        assert_eq!(reason(Some("stale_revision")), "stale_revision");
        assert_eq!(reason(Some("unspecified")), "other");
        assert_eq!(reason(None), "none");
    }
    #[test]
    fn clones_share_counters_and_histograms() {
        let metrics = Metrics::default();
        let clone = metrics.clone();
        clone.catalog_applied(Outcome::Applied, Duration::from_secs(2));
        clone.event_corrected(CorrectionKind::Cancel, Outcome::Accepted, None);
        clone.cardinality_rejected(LimitKind::Groups);
        let text = metrics.encode();
        assert!(text.contains("delcore_catalog_reconciliations_total{outcome=\"applied\"} 1"));
        assert!(text.contains("delcore_catalog_reconciliation_duration_seconds_sum 2"));
        assert!(text.contains("delcore_catalog_reconciliation_duration_seconds_count 1"));
        assert!(text.contains("delcore_metering_corrections_total{kind=\"cancel\",outcome=\"accepted\",reason=\"none\"} 1"));
        assert!(text.contains("delcore_metering_cardinality_rejections_total{limit=\"groups\"} 1"));
        assert!(text.contains("delcore_metering_lag_seconds 0"));
    }
}
